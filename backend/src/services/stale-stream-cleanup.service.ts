import { PrismaClient } from '../generated/client/client.js';
import { prisma as defaultPrisma } from '../lib/db.js';
import { logger } from '../logger.js';

export interface CleanupResult {
  updatedCount: number;
}

/**
 * Transitions ACTIVE streams whose endTime has elapsed to COMPLETED status.
 *
 * Uses a single bulk updateMany for efficiency — O(1) DB round-trips
 * regardless of how many streams have expired.
 */
export class StaleStreamCleanupService {
  private readonly db: PrismaClient;

  constructor(db: PrismaClient = defaultPrisma) {
    this.db = db;
  }

  async markExpiredStreamsCompleted(): Promise<CleanupResult> {
    const now = new Date();

    try {
      const result = await this.db.stream.updateMany({
        where: {
          status: 'ACTIVE',
          endTime: { lt: now },
        },
        data: {
          status: 'COMPLETED',
        },
      });

      logger.info('Stale stream cleanup completed', { updatedCount: result.count });
      return { updatedCount: result.count };
    } catch (error) {
      logger.error('Stale stream cleanup failed', error);
      throw error;
    }
  }
}
