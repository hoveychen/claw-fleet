export type VoiceCue = {
  file: string;
  startFrame: number;
  measuredSeconds: number;
};

// Durations are measured from the generated MP3 files with ffprobe.
export const HORIZONTAL_VOICE: VoiceCue[] = [
  { file: "h-intro", startFrame: 5, measuredSeconds: 2.76 },
  { file: "h-board", startFrame: 95, measuredSeconds: 4.488 },
  { file: "h-usage", startFrame: 245, measuredSeconds: 4.032 },
  { file: "h-decisions", startFrame: 380, measuredSeconds: 5.088 },
  { file: "h-dispatch", startFrame: 560, measuredSeconds: 4.416 },
  { file: "h-handoff", startFrame: 710, measuredSeconds: 3.504 },
  { file: "h-mobile", startFrame: 845, measuredSeconds: 4.992 },
  { file: "h-report", startFrame: 1055, measuredSeconds: 4.224 },
  { file: "h-outro", startFrame: 1200, measuredSeconds: 4.944 },
];

export const VERTICAL_VOICE: Record<string, VoiceCue> = {
  board: { file: "v-board", startFrame: 18, measuredSeconds: 5.544 },
  usage: { file: "v-usage", startFrame: 18, measuredSeconds: 5.208 },
  guard: { file: "v-guard", startFrame: 18, measuredSeconds: 5.448 },
  decide: { file: "v-decide", startFrame: 18, measuredSeconds: 5.64 },
  dispatch: { file: "v-dispatch", startFrame: 18, measuredSeconds: 5.88 },
  handoff: { file: "v-handoff", startFrame: 18, measuredSeconds: 5.544 },
  mobile: { file: "v-mobile", startFrame: 18, measuredSeconds: 5.304 },
  report: { file: "v-report", startFrame: 18, measuredSeconds: 5.088 },
  wiki: { file: "v-wiki", startFrame: 18, measuredSeconds: 5.712 },
};
