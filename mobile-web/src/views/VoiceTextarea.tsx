// Textarea with voice input — for input surfaces that lack a toolbar.
//
// The two composers (new session / continuation) each have an "Attachments · Voice" toolbar,
// and the recording bar takes over that row. But the input fields in decision cards are bare
// textareas: rejection reasons, objections, free-form answers, form fields. They are also
// "natural language spoken to AI", and answering decision cards is the highest-frequency input
// scenario on mobile, so these also need to support voice — and it must use the same interaction
// as the composer, not different gestures here and there.
//
// When idle, the microphone floats in the bottom-right corner (DeepSeek / Kimi position);
// when recording, the recording bar sits at the bottom of the input field: neither requires
// adding a toolbar line to each call site, nor changes their existing layout and styling.
//
// When voice is unavailable (browser unsupported, Android device without GMS), it degrades to
// a plain textarea, without adding extra padding.

import { useFollowTail, useVoiceRecorder } from "../useVoiceRecorder";
import { VoiceBar, VoiceMicButton } from "./VoiceBar";
import styles from "./VoiceTextarea.module.css";

export function VoiceTextarea({
  value,
  onChange,
  className,
  placeholder,
  rows,
  lang = "zh-CN",
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (next: string) => void;
  /** The original textarea class from the call site is passed through as-is; styling is unaffected by this component. */
  className?: string;
  placeholder?: string;
  rows?: number;
  lang?: string;
  "aria-label"?: string;
}) {
  const rec = useVoiceRecorder({ lang, value, onChange });
  const taRef = useFollowTail<HTMLTextAreaElement>(rec.showingPreview, rec.preview);

  return (
    <div className={styles.wrap}>
      <textarea
        ref={taRef}
        className={className}
        placeholder={placeholder}
        value={rec.showingPreview ? rec.preview : value}
        readOnly={rec.showingPreview}
        onChange={(e) => onChange(e.target.value)}
        rows={rows}
        aria-label={ariaLabel}
        // Make room for the microphone in the bottom-right corner, otherwise the last line
        // of text is obscured by the button; when recording, make room for the recording bar at the bottom.
        style={
          rec.active
            ? { paddingBottom: 56 }
            : rec.available
              ? { paddingRight: 48 }
              : undefined
        }
      />
      {rec.active ? (
        <div className={styles.barSlot}>
          <VoiceBar rec={rec} />
        </div>
      ) : (
        rec.available && (
          <div className={styles.slot}>
            <VoiceMicButton rec={rec} />
          </div>
        )
      )}
    </div>
  );
}
