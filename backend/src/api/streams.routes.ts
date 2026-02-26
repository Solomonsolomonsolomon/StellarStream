import { Router, Request, Response } from "express";
import { z } from "zod";
import { StreamService } from "../services/stream.service";
import {
  CurveTypeInput,
  StreamFeeEstimationService,
} from "../services/stream-fee-estimation.service";
import validateRequest from "../middleware/validateRequest";
import stellarAddressSchema from "../validation/stellar";
import asyncHandler from "../utils/asyncHandler";

const router = Router();
const streamService = new StreamService();
const streamFeeEstimationService = new StreamFeeEstimationService();

const getStreamsParamsSchema = z.object({
  address: stellarAddressSchema,
});

const getStreamsQuerySchema = z.object({
  direction: z.enum(["inbound", "outbound"]).optional(),
  status: z.enum(["active", "paused", "completed"]).optional(),
  tokens: z.string().optional(),
});

const estimateFeeBodySchema = z.object({
  sender: stellarAddressSchema,
  receiver: stellarAddressSchema,
  token: stellarAddressSchema,
  totalAmount: z.string().regex(/^\d+$/, {
    message: "totalAmount must be an integer string in stroops.",
  }),
  startTime: z.number().int().positive(),
  endTime: z.number().int().positive(),
  curveType: z.enum(["linear", "exponential"]).default("linear"),
  isSoulbound: z.boolean().default(false),
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

/**
 * POST /api/v1/streams/estimate-fee
 * Estimates Soroban fee (resource + inclusion) for create_stream in XLM.
 */
router.post(
  "/streams/estimate-fee",
  validateRequest({
    body: estimateFeeBodySchema,
  }),
  asyncHandler(async (req: Request, res: Response) => {
    const body = req.body as z.infer<typeof estimateFeeBodySchema>;

    if (body.endTime <= body.startTime) {
      res.status(400).json({
        success: false,
        error: "endTime must be greater than startTime.",
      });
      return;
    }

    const estimate = await streamFeeEstimationService.estimateCreateStreamFee({
      sender: body.sender,
      receiver: body.receiver,
      token: body.token,
      totalAmount: body.totalAmount,
      startTime: body.startTime,
      endTime: body.endTime,
      curveType: body.curveType as CurveTypeInput,
      isSoulbound: body.isSoulbound,
    });

    res.json({
      success: true,
      estimate,
    });
  })
);

export default router;
