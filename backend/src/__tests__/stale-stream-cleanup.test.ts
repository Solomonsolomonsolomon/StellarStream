import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { StaleStreamCleanupService } from "../services/stale-stream-cleanup.service.js";
import { PrismaClient } from "../generated/client/client.js";

// ═══════════════════════════════════════════════════════════════
// Mock helpers
// ═══════════════════════════════════════════════════════════════

/**
 * Builds a minimal Prisma-shaped stub with only the stream.updateMany
 * method, which is all StaleStreamCleanupService needs.
 */
function createMockPrisma(updateManyCount: number): PrismaClient {
  return {
    stream: {
      updateMany: async (_args: unknown) => ({ count: updateManyCount }),
    },
  } as unknown as PrismaClient;
}

function createErrorPrisma(message: string): PrismaClient {
  return {
    stream: {
      updateMany: async (_args: unknown) => {
        throw new Error(message);
      },
    },
  } as unknown as PrismaClient;
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

describe("StaleStreamCleanupService", () => {
  it("should return updatedCount matching the number of rows the DB updated", async () => {
    const service = new StaleStreamCleanupService(createMockPrisma(7));
    const result = await service.markExpiredStreamsCompleted();
    assert.equal(result.updatedCount, 7);
  });

  it("should return 0 when no stale streams exist", async () => {
    const service = new StaleStreamCleanupService(createMockPrisma(0));
    const result = await service.markExpiredStreamsCompleted();
    assert.equal(result.updatedCount, 0);
  });

  it("should re-throw when the database query fails", async () => {
    const service = new StaleStreamCleanupService(
      createErrorPrisma("DB connection lost"),
    );
    await assert.rejects(
      () => service.markExpiredStreamsCompleted(),
      /DB connection lost/,
    );
  });

  it("should pass status ACTIVE and endTime lt filter to updateMany", async () => {
    let capturedArgs: unknown = null;
    const mockPrisma = {
      stream: {
        updateMany: async (args: unknown) => {
          capturedArgs = args;
          return { count: 3 };
        },
      },
    } as unknown as PrismaClient;

    const service = new StaleStreamCleanupService(mockPrisma);
    const before = new Date();
    await service.markExpiredStreamsCompleted();
    const after = new Date();

    const args = capturedArgs as {
      where: { status: string; endTime: { lt: Date } };
      data: { status: string };
    };

    assert.equal(args.where.status, "ACTIVE");
    assert.equal(args.data.status, "COMPLETED");
    // The cutoff timestamp must fall between before and after
    assert.ok(args.where.endTime.lt >= before);
    assert.ok(args.where.endTime.lt <= after);
  });
});
