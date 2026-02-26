import { Router, Request, Response } from "express";
import { prisma } from "../lib/db.js";
import asyncHandler from "../utils/asyncHandler.js";

interface IndexedProposalRow {
  id: string;
  creator: string;
  description: string;
  quorum: number;
  votesFor: number;
  votesAgainst: number;
  txHash: string;
  createdAt: Date;
  updatedAt: Date;
}

const router = Router();

/**
 * GET /api/v1/governance/proposals
 * Returns DAO governance proposals indexed from contract events.
 */
router.get(
  "/governance/proposals",
  asyncHandler(async (_req: Request, res: Response) => {
    const proposals = await prisma.$queryRaw<IndexedProposalRow[]>`
      SELECT
        "id",
        "creator",
        "description",
        "quorum",
        "votesFor",
        "votesAgainst",
        "txHash",
        "createdAt",
        "updatedAt"
      FROM "Proposal"
      ORDER BY "createdAt" DESC
    `;

    res.json({
      success: true,
      count: proposals.length,
      proposals: proposals.map((proposal) => ({
        id: proposal.id,
        creator: proposal.creator,
        description: proposal.description,
        quorum: proposal.quorum,
        votesFor: proposal.votesFor,
        votesAgainst: proposal.votesAgainst,
        txHash: proposal.txHash,
        createdAt: proposal.createdAt,
        updatedAt: proposal.updatedAt,
      })),
    });
  })
);

export default router;
