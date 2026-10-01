import * as z from 'zod';

// 128bit なので先頭文字は 0-7 に限られる。
const ULID_PATTERN = '[0-7][0-9A-HJKMNP-TV-Z]{25}';

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

const INPUT_SOURCES = ['hub', 'terminal'] as const;

/** 送信側 (params) 用。未知の値は拒否する。 */
export const InputSourceSchema = z
  .enum(INPUT_SOURCES)
  .describe('Origin of the input: the controlling app (hub) or a terminal client');

/** 受信側 (イベント data) 用。未知の値は "unknown" として読む。 */
export const ReportedInputSourceSchema = z
  .enum([...INPUT_SOURCES, 'unknown'])
  .catch('unknown')
  .describe(
    'Origin of the input: the controlling app (hub) or a terminal client. The daemon never sends "unknown"; receivers read values they do not know as "unknown".',
  );

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
      'True when events were missed: since is older than the retained range (since < oldest-1), since is ahead of head, or the epoch passed with since differs from the current one. A daemon restart is detected only by comparing epoch.',
    ),
  head: SeqSchema.describe(
    'Latest seq at subscribe time. Replay covers seq <= head; live delivery covers seq > head.',
  ),
  epoch: EpochSchema,
});

export const SubscribeEpochSchema = EpochSchema.describe(
  'Epoch that since belongs to. If it differs from the current epoch, the daemon returns gap: true and replays from the oldest retained item.',
);

export const OkResultSchema = z.looseObject({ ok: z.literal(true) });
