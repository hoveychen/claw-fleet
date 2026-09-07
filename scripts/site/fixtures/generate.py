"""Small, self-contained sample deliverables consumed by the real artifact viewer."""
from pathlib import Path
from html import escape
import json
ROOT = Path(__file__).parent
scenes = json.loads((ROOT / 'scenes.json').read_text())
for lang, scene in scenes.items():
    folder = ROOT / lang
    folder.mkdir(exist_ok=True)
    for i, title in enumerate(scene['artifacts']):
        if i in (3,4):
            (folder / f'{i}.md').write_text(scene['reply'] if i == 4 else f'# {title}\n\n'+ ('24 interviews · 5 customer segments\n\n## Findings\n\n1. Keep work and decisions together.\n2. Show finished deliverables early.\n3. Make mobile review feel effortless.\n\n> “I want to see what is ready, not chase status updates.”\n\n## Next step\nTest the launch story with five customers.' if lang=='en' else '24 场访谈 · 5 类客户\n\n## 研究发现\n\n1. 把工作和需要做的决定放在一起。\n2. 尽早展示完成的成果。\n3. 让手机审阅变得轻松。\n\n> “我想看到什么已经做好，而不是追着问进度。”\n\n## 下一步\n邀请五位客户评审发布叙事。'))
            continue
        bg, fg, accent = [('#f3eedf','#263b32','#e68345'),('#eef4f0','#183d35','#448873'),('#eaddc7','#433d30','#c26541'),('#f4eee4','#3f4335','#6b8755'),('#ecf1f3','#243e4b','#5f90a0'),('#f5eee8','#462f2b','#b85b44'),('#223d34','#fff1cf','#edaf5b'),('#f0efe5','#344233','#73916a')][i]
        if i in (1,5):
            content = f'<div class="metric">{"+18.6%" if i==1 else "+12.0%"}</div><p>{"Revenue growth · September" if lang=="en" else "九月收入增长"}</p><svg viewBox="0 0 620 230" width="100%"><path d="M10 205 L110 177 L210 184 L310 122 L410 137 L510 65 L610 20 L610 230 L10 230Z" fill="{accent}" opacity=".18"/><path d="M10 205 L110 177 L210 184 L310 122 L410 137 L510 65 L610 20" fill="none" stroke="{accent}" stroke-width="5"/></svg><div class="row"><span>APR</span><span>MAY</span><span>JUN</span><span>JUL</span><span>AUG</span><span>SEP</span></div>'
        elif i == 2:
            content = f'<div class="metric" style="font-family:Georgia">Field<br>notes.</div><div class="swatches"><b style="background:#274b39"></b><b style="background:#c26b42"></b><b style="background:#e5c693"></b></div><p>{"A quieter kind of confidence." if lang=="en" else "安静，也有自己的态度。"}</p>'
        elif i == 6:
            content = f'<div class="metric" style="font-size:82px">{"Make room<br>for the work." if lang=="en" else "让好想法<br>成为作品。"}</div><hr><p>AUTUMN / 2026 &nbsp; — &nbsp; STUDIO NOTES 01</p>'
        elif i == 7:
            content = '<div style="display:grid;gap:26px;margin-top:36px">'+''.join(f'<div style="background:{accent};color:white;padding:18px;margin-left:{j*38}px;width:{85-j*12}%">0{j+1} &nbsp; {v}</div>' for j,v in enumerate(['Research','Prototype','Build','Launch'] if lang=='en' else ['研究与访谈','方案与原型','制作与验证','发布与回顾']))+'</div>'
        else:
            content = f'<div class="metric" style="font-family:Georgia">{"Less busy.<br>More made." if lang=="en" else "少一点忙乱。<br>多一些成果。"}</div><hr><div class="row"><div><h3>01</h3>{"The customer story" if lang=="en" else "用户的故事"}</div><div><h3>02</h3>{"The finished work" if lang=="en" else "完成的作品"}</div><div><h3>03</h3>{"The launch plan" if lang=="en" else "发布的计划"}</div></div>'
        html=f'<!doctype html><html lang="{lang}"><meta charset="utf-8"><title>{escape(title)}</title><style>*{{box-sizing:border-box}}body{{margin:0;padding:46px;background:{bg};color:{fg};font:18px/1.4 Arial,sans-serif}}header{{font-size:12px;letter-spacing:3px;border-bottom:1px solid {accent};padding-bottom:18px}}h1{{font-size:26px;font-weight:500;margin:28px 0}}.metric{{font-size:72px;line-height:1.06;font-weight:600;margin:36px 0;color:{fg}}}.row{{display:flex;justify-content:space-between;font-size:14px}}hr{{border:0;border-top:1px solid {accent};margin:32px 0}}.swatches{{display:flex;gap:18px}}.swatches b{{display:block;width:95px;height:75px}}footer{{margin-top:34px;font-size:11px;letter-spacing:1px;opacity:.7}}</style><header>STUDIO / AUTUMN 2026</header><h1>{escape(title)}</h1>{content}<footer>{"ILLUSTRATIVE PROJECT · PREPARED WITH FLEET" if lang=="en" else "示例项目 · 使用 FLEET 制作"}</footer></html>'
        (folder / f'{i}.html').write_text(html)
