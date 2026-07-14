#!/usr/bin/env python3
"""Original nautical/sea-shanty BGM for the Claw Fleet promo.
Pure-Python MIDI (no deps) -> render with fluidsynth + GM soundfont -> mp3.
Fully original composition; free of licensing constraints."""
import struct, sys

TPQ = 480
BPM = 116
US_PER_Q = int(60_000_000 / BPM)

def vlq(n):
    b = [n & 0x7f]; n >>= 7
    while n: b.append((n & 0x7f) | 0x80); n >>= 7
    return bytes(reversed(b))

events = []  # (abs_tick, order, message-bytes)
def add(t, msg, order=1): events.append((t, order, bytes(msg)))
def prog(ch, p): add(0, [0xC0 | ch, p], 0)
def note(ch, pitch, start, dur_beats, vel):
    if pitch is None: return
    st = int(start * TPQ)
    en = int((start + dur_beats) * TPQ)
    add(st, [0x90 | ch, pitch, vel], 1)
    add(en, [0x80 | ch, pitch, 0], 0)

# ---- channels ----
MEL, BASS, PAD, PLUCK, DR = 0, 1, 2, 3, 9
prog(MEL, 21)    # Accordion
prog(BASS, 32)   # Acoustic Bass
prog(PAD, 48)    # String Ensemble
prog(PLUCK, 45)  # Pizzicato Strings

# ---- harmony: D major shanty progression, 1 chord per bar (4/4) ----
# roots + major/minor triad tones
D,E,Fs,G,A,B,Cs = 62,64,66,67,69,71,73
CH = {
 'D':  (50, [62,66,69]),   # D  F# A
 'G':  (43, [67,71,62]),   # G  B  D
 'A':  (45, [69,73,64]),   # A  C# E
 'Bm': (47, [71,62,66]),   # B  D  F#
 'Em': (52, [64,67,71]),   # E  G  B
}
# 8-bar theme progression
THEME = ['D','A','Bm','G','D','A','G','A']
# melody per 8 bars: list of (pitch, dur_beats); None = rest. Bars are 4 beats.
# jaunty, stepwise, chord-tone anchored — original tune.
MELODY = [
  # bar1 D
  (A,0.5),(A,0.5),(B,0.5),(A,0.5),(Fs,1.0),(A,1.0),
  # bar2 A
  (E+12,0.5),(Cs+12,0.5),(A,0.5),(Cs+12,0.5),(E+12,1.0),(A,1.0),
  # bar3 Bm
  (Fs+12,0.5),(E+12,0.5),(D+12,0.5),(Fs+12,0.5),(B,1.0),(D+12,1.0),
  # bar4 G
  (G+12,0.5),(Fs+12,0.5),(E+12,0.5),(D+12,0.5),(B,1.0),(G,1.0),
  # bar5 D
  (A,0.5),(A,0.5),(B,0.5),(Cs+12,0.5),(D+12,1.0),(A,1.0),
  # bar6 A
  (Cs+12,0.5),(D+12,0.5),(E+12,0.5),(Cs+12,0.5),(A,1.0),(E+12,1.0),
  # bar7 G
  (D+12,0.5),(B,0.5),(G,0.5),(B,0.5),(D+12,1.0),(B,1.0),
  # bar8 A -> turnaround
  (Cs+12,0.5),(E+12,0.5),(A+12,0.5),(E+12,0.5),(A,1.0),(A,0.5),(Cs+12,0.5),
]

def bass_bar(ch, chord_root, bar_start, tuba=False):
    # oom-pah: root beat1&3, octave up beat2&4
    add(0,[0])  # noop guard (never used)
def lay_bass(root, bar_start, vel=88):
    note(BASS, root, bar_start+0, 0.9, vel)
    note(BASS, root+12, bar_start+1, 0.6, vel-14)
    note(BASS, root+7, bar_start+2, 0.9, vel)
    note(BASS, root+12, bar_start+3, 0.6, vel-14)
def lay_pad(tones, bar_start, vel=42):
    for p in tones: note(PAD, p, bar_start, 4.0, vel)
def lay_pluck(tones, bar_start, vel=60):
    # offbeat pizzicato bounce on the "&" of each beat
    for i in range(4):
        p = tones[(i) % len(tones)] + 12
        note(PLUCK, p, bar_start + i + 0.5, 0.35, vel)
def drum(bar_start):
    # kick 1&3, snare 2&4, tambourine on eighths
    for beat,inst,v in [(0,36,100),(1,38,90),(2,36,100),(3,38,90)]:
        note(DR, inst, bar_start+beat, 0.2, v)
    for i in range(8):
        note(DR, 54, bar_start + i*0.5, 0.1, 46)  # tambourine

def play_theme(bar0, mel_vel=96, with_drums=True, with_mel=True):
    # harmony/bass/pad/pluck for 8 bars
    for i,name in enumerate(THEME):
        root, tones = CH[name]
        bs = bar0 + i*4
        lay_bass(root, bs)
        lay_pad(tones, bs)
        lay_pluck(tones, bs)
        if with_drums: drum(bs)
    if with_mel:
        t = bar0
        for pitch,dur in MELODY:
            note(MEL, pitch, t, dur*0.96, mel_vel)
            t += dur

# ---- arrangement (~66s). bar=4 beats; at 116bpm bar≈2.07s ----
# intro 4 bars (bass+pad+pluck, no mel), then 3 theme passes, outro 2 bars
bar = 0.0
# intro: 4 bars, gentle build, no melody, drums enter bar3
for i in range(4):
    name = ['D','D','A','A'][i]
    root,tones = CH[name]; bs = i*4
    lay_bass(root,bs, vel=74); lay_pad(tones,bs,vel=38); lay_pluck(tones,bs,vel=48)
    if i>=2: drum(bs)
intro_bars = 4
# 4 full passes of the 8-bar theme (last pass drops melody density for a lift)
play_theme(intro_bars*4 + 0,   mel_vel=92)
play_theme(intro_bars*4 + 32,  mel_vel=100)
play_theme(intro_bars*4 + 64,  mel_vel=104)
play_theme(intro_bars*4 + 96,  mel_vel=106)
# outro: final D chord ring + melody resolve
end_bar = (intro_bars + 32)*4
root,tones = CH['D']
lay_bass(root, end_bar, vel=84); lay_pad(tones, end_bar, vel=48)
note(MEL, 62+12, end_bar, 2.0, 100); note(MEL, 69, end_bar, 2.0, 84); note(MEL, 66, end_bar, 2.0, 84)
drum(end_bar)

# ---- serialize (format 0, single track, multi-channel) ----
events.sort(key=lambda e:(e[0], e[1]))
trk = b''; last = 0
# tempo meta
trk += vlq(0) + b'\xff\x51\x03' + struct.pack('>I', US_PER_Q)[1:]
for t,order,msg in events:
    trk += vlq(t-last) + msg; last = t
trk += vlq(0) + b'\xff\x2f\x00'
head = b'MThd' + struct.pack('>I',6) + struct.pack('>HHH',0,1,TPQ)
open('audio-build/bgm.mid','wb').write(head + b'MTrk' + struct.pack('>I',len(trk)) + trk)
print("bgm.mid written, %d events, last tick %d (~%.1fs)" % (len(events), last, last/TPQ*60/BPM))
