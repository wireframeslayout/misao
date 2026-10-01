import * as z from 'zod';

const ULID_PATTERN = '[0-9A-HJKMNP-TV-Z]{26}';

export const UlidSchema = z
  .string()
  .regex(new RegExp(`^${ULID_PATTERN}$`))
  .describe('ULID (Crockford base32, 26 characters)');

export const EpochSchema = UlidSchema.describe(
  'ULID that changes on every daemon start. When it changes, seq has been rewound: the client must discard its local since.',
);

export const PaneIdSchema = z
  .string()
  .regex(new RegExp(`^p_${ULID_PATTERN}$`))
  .describe('Pane id: "p_" followed by a ULID');

export const WindowIdSchema = z
  .string()
  .regex(new RegExp(`^w_${ULID_PATTERN}$`))
  .describe('Window id: "w_" followed by a ULID');

export const WorkspaceNameSchema = z.string().min(1).describe('Workspace name');

export const WindowNameSchema = z.string().min(1).describe('Window name');

export const ClientIdSchema = z.string().min(1).describe('Client identifier chosen by the client');

export const SeqSchema = z
  .int()
  .min(0)
  .describe(
    'Sequence number. Increases monotonically from 1 per stream. Streams: raw output per pane, lines per pane, and daemon-wide events.',
  );

export const TsSchema = z
  .iso.datetime()
  .describe('Time recorded by the daemon (ISO 8601, UTC)');

export const DimensionSchema = z.int().positive().describe('Positive integer (columns or rows)');

export const SinceSchema = SeqSchema.describe(
  'Last seq already received. Replays seq > since, then switches to live. Omit for live only.',
);

export const LabelsSchema = z
  .record(z.string(), z.string())
  .describe(
    'Free-form labels. Recommended keys: owner, task, agent, origin, windowId. Recommended origin values: "hub", "terminal".',
  );

export const SubscribeResultSchema = z.looseObject({
  gap: z
    .boolean()
    .describe(
      'True when since is older than the retained range (since < oldest-1) or ahead of head (the daemon restarted): events were missed.',
    ),
  head: SeqSchema.describe(
    'Latest seq at subscribe time. Replay covers seq <= head; live delivery covers seq > head.',
  ),
  epoch: EpochSchema,
});

export const OkResultSchema = z.looseObject({ ok: z.literal(true) });
