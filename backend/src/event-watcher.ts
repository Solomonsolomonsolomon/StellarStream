/**
 * Core event watcher service for Stellar blockchain
 */

import { SorobanRpc } from "@stellar/stellar-sdk";
import { EventWatcherConfig, WatcherState, ParsedContractEvent } from "./types";
import { logger } from "./logger";
import { parseContractEvent, extractEventType } from "./event-parser";
import { scValToNative, xdr } from "@stellar/stellar-sdk";
import { PrismaClient } from "./generated/client/client.js";

// @ts-ignore
const prisma = new PrismaClient();

export class EventWatcher {
  private server: SorobanRpc.Server;
  private config: EventWatcherConfig;
  private state: WatcherState;
  private isShuttingDown: boolean = false;
  private pollTimeout?: NodeJS.Timeout;

  constructor(config: EventWatcherConfig) {
    this.config = config;
    this.server = new SorobanRpc.Server(config.rpcUrl, {
      allowHttp: config.rpcUrl.startsWith("http://"),
    });

    this.state = {
      lastProcessedLedger: 0,
      isRunning: false,
      errorCount: 0,
    };

    logger.info("EventWatcher initialized", {
      rpcUrl: config.rpcUrl,
      contractId: config.contractId,
      pollInterval: config.pollIntervalMs,
    });
  }

  /**
   * Start the event watcher loop
   */
  async start(): Promise<void> {
    if (this.state.isRunning) {
      logger.warn("EventWatcher is already running");
      return;
    }

    this.state.isRunning = true;
    this.isShuttingDown = false;
    logger.info("EventWatcher started");

    // Get initial ledger position
    await this.initializeCursor();

    // Start polling loop
    await this.pollLoop();
  }

  /**
   * Stop the event watcher gracefully
   */
  async stop(): Promise<void> {
    if (!this.state.isRunning) {
      return;
    }

    logger.info("Stopping EventWatcher...");
    this.isShuttingDown = true;
    this.state.isRunning = false;

    if (this.pollTimeout) {
      clearTimeout(this.pollTimeout);
    }

    logger.info("EventWatcher stopped", {
      lastProcessedLedger: this.state.lastProcessedLedger,
      totalErrors: this.state.errorCount,
    });
  }

  /**
   * Initialize cursor to latest ledger
   */
  private async initializeCursor(): Promise<void> {
    try {
      const latestLedger = await this.server.getLatestLedger();
      this.state.lastProcessedLedger = latestLedger.sequence;
      logger.info("Cursor initialized", {
        startingLedger: this.state.lastProcessedLedger,
      });
    } catch (error) {
      logger.error("Failed to initialize cursor", error);
      // Start from 0 if we can't get latest ledger
      this.state.lastProcessedLedger = 0;
    }
  }

  /**
   * Main polling loop with error handling and backoff
   */
  private async pollLoop(): Promise<void> {
    while (this.state.isRunning && !this.isShuttingDown) {
      try {
        await this.fetchAndProcessEvents();
        this.state.errorCount = 0; // Reset error count on success

        // Wait before next poll
        await this.sleep(this.config.pollIntervalMs);
      } catch (error) {
        this.state.errorCount++;
        this.state.lastError = error instanceof Error ? error : new Error(String(error));

        logger.error("Error in poll loop", error, {
          errorCount: this.state.errorCount,
          lastProcessedLedger: this.state.lastProcessedLedger,
        });

        // Exponential backoff on errors
        const backoffDelay = Math.min(
          this.config.retryDelayMs * Math.pow(2, this.state.errorCount - 1),
          30000 // Max 30 seconds
        );

        logger.info(`Retrying in ${backoffDelay}ms...`);
        await this.sleep(backoffDelay);

        // Stop if too many consecutive errors
        if (this.state.errorCount >= this.config.maxRetries) {
          logger.error("Max retries exceeded, stopping watcher");
          await this.stop();
        }
      }
    }
  }

  /**
   * Fetch events from Stellar RPC and process them
   */
  private async fetchAndProcessEvents(): Promise<void> {
    const startLedger = this.state.lastProcessedLedger + 1;

    logger.debug("Fetching events", { startLedger });

    const response = await this.server.getEvents({
      startLedger,
      filters: [
        {
          type: "contract",
          contractIds: [this.config.contractId],
        },
      ],
      limit: 100, // Process up to 100 events per poll
    });

    if (!response.events || response.events.length === 0) {
      logger.debug("No new events found");

      // Update cursor to latest ledger even if no events
      const latestLedger = await this.server.getLatestLedger();
      this.state.lastProcessedLedger = latestLedger.sequence;
      return;
    }

    logger.info(`Found ${response.events.length} new events`);

    // Process each event
    for (const event of response.events) {
      await this.processEvent(event);

      // Update cursor after each event
      if (event.ledger > this.state.lastProcessedLedger) {
        this.state.lastProcessedLedger = event.ledger;
      }
    }

    logger.debug("Events processed", {
      count: response.events.length,
      lastProcessedLedger: this.state.lastProcessedLedger,
    });
  }

  /**
   * Process a single event
   */
  private async processEvent(event: SorobanRpc.Api.EventResponse): Promise<void> {
    const parsed = parseContractEvent(event);

    if (!parsed) {
      logger.warn("Skipping unparseable event", { eventId: event.id });
      return;
    }

    const eventType = extractEventType(parsed.topics);

    // Log the raw event to console (as per acceptance criteria)
    logger.event(eventType, {
      id: parsed.id,
      type: parsed.type,
      ledger: parsed.ledger,
      ledgerClosedAt: parsed.ledgerClosedAt,
      contractId: parsed.contractId,
      txHash: parsed.txHash,
      topics: parsed.topics,
      value: parsed.value,
      inSuccessfulContractCall: parsed.inSuccessfulContractCall,
    });

    // Here you can add custom handlers for specific event types
    await this.handleEventByType(eventType, parsed);
  }

  /**
   * Handle events based on their type
   * Extend this method to add custom business logic
   */
  private async handleEventByType(
    eventType: string,
    event: ParsedContractEvent
  ): Promise<void> {
    switch (eventType) {
      case "stream_created":
        logger.info("Stream created event detected", {
          txHash: event.txHash,
          ledger: event.ledger,
        });

        try {
          let sender = "";
          let receiver = "";
          let amount = "0";
          let duration = 0;

          // Attempt to extract from Topics (using requested fromXdr and scValToNative)
          if (event.topics && event.topics.length > 1) {
            const senderVal = xdr.ScVal.fromXDR(event.topics[1], "base64");
            sender = String(scValToNative(senderVal));
          }

          // Extract further data from the parsed value
          const data = event.value as any;
          if (Array.isArray(data)) {
            // Assume [receiver, amount, duration] or similar
            receiver = data[0] ? String(data[0]) : "";
            amount = data[1] ? String(data[1]) : "0";
            duration = data[2] ? Number(data[2]) : 0;
          } else if (typeof data === "object" && data !== null) {
            // Assume named fields struct
            receiver = data.receiver ? String(data.receiver) : "";
            amount = data.amount ? String(data.amount) : "0";
            duration = data.duration ? Number(data.duration) : 0;
            if (!sender && data.sender) {
              sender = String(data.sender);
            }
          }

          await prisma.stream.create({
            data: {
              txHash: event.txHash,
              sender,
              receiver,
              amount,
              duration,
            },
          });
          logger.info("Stream successfully saved to Prisma DB", { txHash: event.txHash });
        } catch (error) {
          logger.error("Failed to decode or save StreamCreated event", error);
        }
        break;

      case "stream_withdrawn":
        logger.info("Withdrawal event detected", {
          txHash: event.txHash,
          ledger: event.ledger,
        });
        break;

      case "stream_cancelled":
        logger.info("Cancellation event detected", {
          txHash: event.txHash,
          ledger: event.ledger,
        });
        break;

      default:
        logger.debug("Unhandled event type", { eventType });
    }
  }

  /**
   * Get current watcher state
   */
  getState(): Readonly<WatcherState> {
    return { ...this.state };
  }

  /**
   * Sleep utility
   */
  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      this.pollTimeout = setTimeout(resolve, ms);
    });
  }
}
