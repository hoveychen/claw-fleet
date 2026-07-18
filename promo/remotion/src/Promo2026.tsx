import React from "react";
import {
  AbsoluteFill,
  Audio,
  Img,
  Loop,
  OffthreadVideo,
  Sequence,
  Series,
  interpolateColors,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { clamp01, easeOut, lerp } from "./helpers";
import { FONT, T } from "./tokens";
import { HORIZONTAL_VOICE, VERTICAL_VOICE, type VoiceCue } from "./voiceTiming";

const SHADOW = "0 24px 70px rgba(32,28,18,.22)";

const enter = (frame: number, fps: number, delay = 0) =>
  spring({ frame: frame - delay, fps, config: { damping: 15, mass: 0.7 } });

const voiceEnvelope = (frame: number, cue: VoiceCue, fps: number) => {
  const start = cue.startFrame;
  const end = start + Math.ceil(cue.measuredSeconds * fps);
  return Math.min(clamp01((frame - (start - 8)) / 8), clamp01(((end + 10) - frame) / 10));
};

const bgmVolume = (frame: number, cues: VoiceCue[], fps: number, idle = 0.34, ducked = 0.14) => {
  const duck = cues.reduce((level, cue) => Math.max(level, voiceEnvelope(frame, cue, fps)), 0);
  return lerp(idle, ducked, duck);
};

const VoiceTracks: React.FC<{ cues: VoiceCue[] }> = ({ cues }) => (
  <>
    {cues.map((cue) => (
      <Sequence key={cue.file} from={cue.startFrame}>
        <Audio src={staticFile(`audio/vo-2026/${cue.file}.mp3`)} volume={0.96} />
      </Sequence>
    ))}
  </>
);

const SoundCue: React.FC<{ file: "alert" | "click" | "success" | "whoosh"; frame: number; volume?: number }> = ({ file, frame, volume = 0.55 }) => (
  <Sequence from={frame}>
    <Audio src={staticFile(`audio/sfx/${file}.wav`)} volume={volume} />
  </Sequence>
);

const Brand: React.FC<{ compact?: boolean }> = ({ compact }) => (
  <div style={{ display: "flex", alignItems: "center", gap: compact ? 14 : 20 }}>
    <Img
      src={staticFile("icon.png")}
      style={{ width: compact ? 58 : 76, height: compact ? 58 : 76, borderRadius: 18 }}
    />
    <div style={{ fontFamily: FONT.display, fontSize: compact ? 35 : 48, fontWeight: 700 }}>Claw Fleet</div>
  </div>
);

const Paper: React.FC<{ children: React.ReactNode; dark?: boolean }> = ({ children, dark }) => (
  <AbsoluteFill
    style={{
      background: dark ? T.night : T.paper,
      color: dark ? T.nightText : T.ink,
      fontFamily: FONT.body,
      overflow: "hidden",
    }}
  >
    {children}
  </AbsoluteFill>
);

const Eyebrow: React.FC<{ children: React.ReactNode; dark?: boolean }> = ({ children, dark }) => (
  <div
    style={{
      fontFamily: FONT.mono,
      fontSize: 24,
      fontWeight: 700,
      letterSpacing: ".08em",
      textTransform: "uppercase",
      color: dark ? T.coralNight : T.coral,
    }}
  >
    {children}
  </div>
);

const Window: React.FC<{
  src: string;
  style?: React.CSSProperties;
  videoStyle?: React.CSSProperties;
  startFrom?: number;
}> = ({ src, style, videoStyle, startFrom = 0 }) => (
  <div
    style={{
      position: "absolute",
      overflow: "hidden",
      borderRadius: 22,
      border: `2px solid ${T.borderStrong}`,
      background: T.card,
      boxShadow: SHADOW,
      ...style,
    }}
  >
    <OffthreadVideo
      muted
      src={staticFile(src)}
      startFrom={startFrom}
      style={{ width: "100%", height: "100%", objectFit: "cover", ...videoStyle }}
    />
  </div>
);

const Intro: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const a = enter(frame, fps, 4);
  const word = clamp01((frame - 35) / 14);
  const mascot = enter(frame, fps, 44);
  return (
    <Paper>
      <div style={{ position: "absolute", top: 70, left: 86 }}><Brand compact /></div>
      <div style={{ position: "absolute", left: 120, top: 270, width: 1200, opacity: a, transform: `translateY(${lerp(45, 0, a)}px)` }}>
        <Eyebrow>Mission control for coding agents</Eyebrow>
        <div style={{ marginTop: 22, fontFamily: FONT.display, fontSize: 112, lineHeight: .98, fontWeight: 650, letterSpacing: "-.04em" }}>
          Your agents are busy.
          <br />Are they a <span style={{ color: interpolateColors(word, [0, 1], [T.ink, T.coral]) }}>team?</span>
        </div>
      </div>
      <Img
        src={staticFile("mascot-captain.png")}
        style={{ position: "absolute", width: 410, height: 410, objectFit: "cover", borderRadius: "50%", right: 95, bottom: -25, opacity: mascot, transform: `scale(${mascot}) rotate(${lerp(8, 0, mascot)}deg)`, boxShadow: SHADOW }}
      />
      <div style={{ position: "absolute", right: 350, bottom: 300, width: 400, border: `3px solid ${T.ink}`, borderRadius: 24, background: T.card, padding: "22px 28px", fontSize: 28, fontWeight: 650, opacity: mascot }}>
        Nine terminals is not a management strategy.
      </div>
    </Paper>
  );
};

const Overview: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = enter(frame, fps, 4);
  return (
    <Paper>
      <div style={{ position: "absolute", top: 55, left: 82 }}><Eyebrow>One live board</Eyebrow></div>
      <div style={{ position: "absolute", top: 92, left: 82, fontFamily: FONT.display, fontSize: 76, fontWeight: 650 }}>Every agent. One bridge.</div>
      <Window src="footage/t1-board.mp4" style={{ left: 76, right: 76, top: 205, bottom: 62, opacity: s, transform: `translateY(${lerp(70, 0, s)}px) scale(${lerp(.96, 1, s)})` }} />
    </Paper>
  );
};

const SideFeature: React.FC<{ eyebrow: string; title: string; copy: string; src: string; align?: "left" | "right" }> = ({ eyebrow, title, copy, src, align = "left" }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = enter(frame, fps, 3);
  const textLeft = align === "left" ? 90 : 1270;
  const windowLeft = align === "left" ? 650 : 80;
  return (
    <Paper>
      <div style={{ position: "absolute", top: 235, left: textLeft, width: 560, opacity: s, transform: `translateX(${lerp(align === "left" ? -45 : 45, 0, s)}px)` }}>
        <Eyebrow>{eyebrow}</Eyebrow>
        <div style={{ marginTop: 20, fontFamily: FONT.display, fontSize: 76, lineHeight: 1.02, fontWeight: 650 }}>{title}</div>
        <div style={{ marginTop: 28, fontSize: 30, lineHeight: 1.42, color: T.inkSecondary }}>{copy}</div>
      </div>
      <Window src={src} style={{ left: windowLeft, top: 150, width: 1180, height: 760, opacity: s, transform: `translateX(${lerp(align === "left" ? 60 : -60, 0, s)}px)` }} />
    </Paper>
  );
};

const Decisions: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = enter(frame, fps, 2);
  const second = enter(frame, fps, 76);
  return (
    <Paper>
      <div style={{ position: "absolute", left: 85, top: 62 }}><Eyebrow>Decision inbox</Eyebrow></div>
      <div style={{ position: "absolute", left: 85, top: 100, fontFamily: FONT.display, fontSize: 72, fontWeight: 650 }}>Risk explained. Choices attached.</div>
      <Window src="footage/t3-guard.mp4" style={{ left: 80, top: 230, width: 850, height: 700, opacity: s, transform: `translateX(${lerp(-70, 0, s)}px)` }} />
      <Window src="footage/t4-ask.mp4" style={{ right: 80, top: 230, width: 850, height: 700, opacity: second, transform: `translateX(${lerp(70, 0, second)}px)` }} />
      <div style={{ position: "absolute", left: 105, bottom: 70, fontFamily: FONT.mono, fontSize: 22, color: T.green }}>command guard</div>
      <div style={{ position: "absolute", right: 105, bottom: 70, fontFamily: FONT.mono, fontSize: 22, color: T.coral, opacity: second }}>structured answer</div>
    </Paper>
  );
};

const MobileHero: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const phone = enter(frame, fps, 4);
  const zoom = enter(frame, fps, 38);
  return (
    <Paper dark>
      <div style={{ position: "absolute", left: 80, top: 55 }}><Eyebrow dark>web-mobile</Eyebrow></div>
      <div style={{ position: "absolute", left: 80, top: 95, width: 670, fontFamily: FONT.display, fontSize: 76, lineHeight: 1.03, fontWeight: 650 }}>
        The bridge fits in your pocket.
      </div>
      <div style={{ position: "absolute", left: 92, top: 345, width: 430, height: 660, borderRadius: 34, overflow: "hidden", border: "10px solid #050607", boxShadow: "0 28px 90px #000", opacity: phone, transform: `translateY(${lerp(60, 0, phone)}px)` }}>
        <OffthreadVideo muted src={staticFile("footage/t7-mobile.mp4")} style={{ width: "100%", height: "100%", objectFit: "cover", objectPosition: "top" }} />
      </div>
      <div style={{ position: "absolute", left: 600, right: 70, top: 250, height: 720, borderRadius: 26, overflow: "hidden", border: "2px solid rgba(255,255,255,.18)", background: "#2b2c30", boxShadow: "0 30px 90px #000", opacity: zoom, transform: `translateX(${lerp(75, 0, zoom)}px)` }}>
        <OffthreadVideo muted src={staticFile("footage/t7-mobile.mp4")} style={{ position: "absolute", left: "50%", top: 0, width: 1050, height: "auto", transform: "translateX(-50%)", objectFit: "contain" }} />
      </div>
      <div style={{ position: "absolute", right: 90, bottom: 55, fontFamily: FONT.mono, fontSize: 24, color: T.coralNight, opacity: zoom }}>same live card · magnified</div>
    </Paper>
  );
};

const ReportWiki: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const a = enter(frame, fps, 3);
  const b = enter(frame, fps, 44);
  return (
    <Paper>
      <div style={{ position: "absolute", left: 85, top: 58 }}><Eyebrow>Work becomes memory</Eyebrow></div>
      <div style={{ position: "absolute", left: 85, top: 100, fontFamily: FONT.display, fontSize: 72, fontWeight: 650 }}>The standup writes itself. The lesson stays.</div>
      <Window src="footage/t8-report.mp4" style={{ left: 75, top: 225, width: 850, height: 700, opacity: a, transform: `translateY(${lerp(55, 0, a)}px)` }} />
      <Window src="footage/t9-wiki.mp4" style={{ right: 75, top: 225, width: 850, height: 700, opacity: b, transform: `translateY(${lerp(55, 0, b)}px)` }} />
    </Paper>
  );
};

const Outro: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = enter(frame, fps, 5);
  return (
    <Paper dark>
      <Img src={staticFile("mascot-cockpit.png")} style={{ position: "absolute", inset: 0, width: "100%", height: "100%", objectFit: "cover", opacity: .48 }} />
      <AbsoluteFill style={{ background: "linear-gradient(90deg,rgba(8,9,10,.93),rgba(8,9,10,.45))" }} />
      <div style={{ position: "absolute", left: 120, top: 245, width: 1050, opacity: s, transform: `translateY(${lerp(35, 0, s)}px)` }}>
        <Brand />
        <div style={{ marginTop: 34, fontFamily: FONT.display, fontSize: 92, lineHeight: 1.03, fontWeight: 650 }}>Run the fleet.<br /><span style={{ color: T.coralNight }}>Keep your mind.</span></div>
        <div style={{ marginTop: 36, fontFamily: FONT.mono, fontSize: 26, color: T.nightDim }}>github.com/hoveychen/claw-fleet · free & open source</div>
      </div>
    </Paper>
  );
};

export const ClawFleetPromo2026: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  return (
    <>
      <Audio src={staticFile("audio/bgm.mp3")} volume={() => bgmVolume(frame, HORIZONTAL_VOICE, fps)} />
      <VoiceTracks cues={HORIZONTAL_VOICE} />
      {[90, 240, 375, 555, 705, 840, 1050, 1200].map((cueFrame) => <SoundCue key={cueFrame} file="whoosh" frame={cueFrame} volume={0.38} />)}
      <SoundCue file="alert" frame={407} volume={0.58} />
      <SoundCue file="click" frame={500} />
      <SoundCue file="click" frame={665} />
      <SoundCue file="click" frame={930} />
      <SoundCue file="success" frame={985} volume={0.62} />
      <SoundCue file="success" frame={1210} volume={0.52} />
      <Series>
        <Series.Sequence durationInFrames={90}><Intro /></Series.Sequence>
        <Series.Sequence durationInFrames={150}><Overview /></Series.Sequence>
        <Series.Sequence durationInFrames={135}><SideFeature eyebrow="Spend without surprises" title="See the meter before it bites." copy="Live token speed, cycle usage, and a model-by-model receipt — in the same place as the work." src="footage/t2-usage.mp4" /></Series.Sequence>
        <Series.Sequence durationInFrames={180}><Decisions /></Series.Sequence>
        <Series.Sequence durationInFrames={150}><SideFeature eyebrow="Dispatch" title="One composer. A whole fleet." copy="Pick the workspace, write the job, and send another agent without opening terminal number ten." src="footage/t5-dispatch.mp4" align="right" /></Series.Sequence>
        <Series.Sequence durationInFrames={135}><SideFeature eyebrow="Handoff" title="The context ends. The mission does not." copy="A fresh agent receives the plan, the exact next task, and the gotchas worth remembering." src="footage/t6-chains.mp4" /></Series.Sequence>
        <Series.Sequence durationInFrames={210}><MobileHero /></Series.Sequence>
        <Series.Sequence durationInFrames={150}><ReportWiki /></Series.Sequence>
        <Series.Sequence durationInFrames={150}><Outro /></Series.Sequence>
      </Series>
    </>
  );
};

export type VerticalTopic = {
  id: string;
  eyebrow: string;
  hook: string;
  benefit: string;
  src: string;
  position: string;
  loopFrames: number;
  mobile?: boolean;
};

export const VERTICAL_TOPICS: VerticalTopic[] = [
  { id: "board", eyebrow: "01 · live board", hook: "Nine agents.\nOne place to look.", benefit: "Status, subagents, speed, cost — live.", src: "footage/t1-board.mp4", position: "22% center", loopFrames: 168 },
  { id: "usage", eyebrow: "02 · usage", hook: "The bill should not\nbe the plot twist.", benefit: "See the burn rate before the window resets.", src: "footage/t2-usage.mp4", position: "50% center", loopFrames: 176 },
  { id: "guard", eyebrow: "03 · command guard", hook: "It asked to run\nwhat, exactly?", benefit: "Risk explained before the command runs.", src: "footage/t3-guard.mp4", position: "50% center", loopFrames: 162 },
  { id: "decide", eyebrow: "04 · decision cards", hook: "Stop answering\nin the terminal.", benefit: "Review context. Tap a choice. Keep moving.", src: "footage/t4-ask.mp4", position: "50% center", loopFrames: 198 },
  { id: "dispatch", eyebrow: "05 · dispatch", hook: "Another task?\nNot another terminal.", benefit: "Launch the next agent from one composer.", src: "footage/t5-dispatch.mp4", position: "62% center", loopFrames: 236 },
  { id: "handoff", eyebrow: "06 · handoff", hook: "Context full.\nMission unfinished.", benefit: "Pass the plan, next step, and gotchas forward.", src: "footage/t6-chains.mp4", position: "24% center", loopFrames: 186 },
  { id: "mobile", eyebrow: "07 · web-mobile", hook: "Lunch break.\nAgent still waiting.", benefit: "Approve the live decision from your phone.", src: "footage/t7-mobile.mp4", position: "center top", loopFrames: 168, mobile: true },
  { id: "report", eyebrow: "08 · daily report", hook: "Standup in ten.\nWhat shipped?", benefit: "Fleet turns the day into a useful report.", src: "footage/t8-report.mp4", position: "76% center", loopFrames: 164 },
  { id: "wiki", eyebrow: "09 · wiki", hook: "Solved once.\nRemember forever.", benefit: "Reports become linked, searchable knowledge.", src: "footage/t9-wiki.mp4", position: "72% center", loopFrames: 228 },
];

export const ClawFleetVertical: React.FC<{ topic: VerticalTopic }> = ({ topic }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const hookIn = enter(frame, fps, 2);
  const footageIn = enter(frame, fps, 54);
  const benefitIn = enter(frame, fps, 454);
  const cta = clamp01((frame - 548) / 16);
  const voice = VERTICAL_VOICE[topic.id];
  return (
    <>
      <Audio src={staticFile("audio/bgm.mp3")} volume={() => bgmVolume(frame, [voice], fps, 0.72, 0.14)} />
      <VoiceTracks cues={[voice]} />
      <SoundCue file="whoosh" frame={52} volume={0.42} />
      {(topic.id === "guard" || topic.id === "mobile") && <SoundCue file="alert" frame={280} volume={0.5} />}
      {(topic.id === "decide" || topic.id === "dispatch" || topic.id === "mobile") && <SoundCue file="click" frame={320} />}
      <SoundCue file="whoosh" frame={454} volume={0.3} />
      <SoundCue file="success" frame={550} volume={0.54} />
      <Paper dark>
        <div style={{ position: "absolute", top: 92, left: 72, right: 150, opacity: hookIn, transform: `translateY(${lerp(40, 0, hookIn)}px)` }}>
          <Eyebrow dark>{topic.eyebrow}</Eyebrow>
          <div style={{ marginTop: 26, whiteSpace: "pre-line", fontFamily: FONT.display, fontSize: 86, lineHeight: 1.02, fontWeight: 650, letterSpacing: "-.025em" }}>{topic.hook}</div>
        </div>
        <div style={{ position: "absolute", left: 60, right: 130, top: 430, height: 910, borderRadius: 32, overflow: "hidden", border: "2px solid rgba(255,255,255,.18)", background: topic.mobile ? T.paper : "#1c1d20", boxShadow: "0 30px 100px #000", opacity: footageIn, transform: `translateY(${lerp(65, 0, footageIn)}px)` }}>
          <Loop durationInFrames={topic.loopFrames}>
            <OffthreadVideo muted src={staticFile(topic.src)} style={{ width: "100%", height: "100%", objectFit: "cover", objectPosition: topic.position }} />
          </Loop>
        </div>
        <div style={{ position: "absolute", left: 78, right: 155, top: 1390, borderLeft: `8px solid ${T.coralNight}`, padding: "10px 0 10px 28px", fontSize: 39, lineHeight: 1.28, fontWeight: 650, opacity: benefitIn, transform: `translateX(${lerp(-35, 0, benefitIn)}px)` }}>{topic.benefit}</div>
        <div style={{ position: "absolute", left: 75, right: 155, bottom: 220, display: "flex", justifyContent: "space-between", alignItems: "center", opacity: cta }}>
          <Brand compact />
          <div style={{ fontFamily: FONT.mono, fontSize: 24, color: T.coralNight }}>free · open source</div>
        </div>
      </Paper>
    </>
  );
};
