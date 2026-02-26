import { Router, Request, Response } from "express";
import { z } from "zod";
import { StreamService } from "../services/stream.service";
import validateRequest from "../middleware/validateRequest";
import stellarAddressSchema from "../validation/stellar";
import asyncHandler from "../utils/asyncHandler";

const router = Router();
const streamService = new StreamService();

const getStreamsParamsSchema = z.object({
  address: stellarAddressSchema,
});

const getStreamsQuerySchema = z.object({
  direction: z.enum(["inbound", "outbound"]).optional(),
  status: z.enum(["active", "paused", "completed"]).optional(),
  tokens: z.string().optional(),
});

/**
 * GET /api/v1/streams/:address
 * Returns streams for a given address with optional filtering
 * Query params:
 *   - direction: inbound | outbound (optional)
 *   - status: active | paused | completed (optional)
 *   - tokens: comma-separated token addresses (optional)
 */
router.get(
  "/streams/:address",
  validateRequest({
    params: getStreamsParamsSchema,
    query: getStreamsQuerySchema,
  }),
  asyncHandler(async (req: Request, res: Response) => {
    const { address } = req.params;
    const { direction, status, tokens } = req.query as z.infer<
      typeof getStreamsQuerySchema
    >;

    const filters = {
      ...(direction ? { direction } : {}),
      ...(status ? { status } : {}),
      ...(typeof tokens === "string" && tokens.length > 0
        ? { tokenAddresses: tokens.split(",").map((t) => t.trim()) }
        : {}),
    };

    const streams = await streamService.getStreamsForAddress(
      address,
      filters,
    );

    res.json({
      success: true,
      address,
      count: streams.length,
      filters,
      streams,
    });
  })
);

export default router;