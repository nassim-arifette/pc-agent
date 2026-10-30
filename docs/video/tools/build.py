import json, os
D = json.load(open('timing.json'))
S = {s['sid']: s for s in D['sched']}
W = D['words']
TOTAL = D['total']
OFF = 0.0  # hi-res screencast clips start exactly at the capture clock
_MARKS = {}
def M(name, key):
    if name not in _MARKS: _MARKS[name] = json.load(open(f'hires/{name}.marks.json'))
    return _MARKS[name][key]
def st(sid): return S[sid]['start']
def en(sid): return S[sid]['start'] + S[sid]['dur']
def word(sid, text, n=0):
    hits = [w for w in W if w['sid'] == sid and w['text'].strip('.,?:!').lower() == text.lower()]
    return hits[n]['start']

COMMON_CSS = """
#root { position:absolute; inset:0; overflow:hidden; font-family: Inter, sans-serif; color:#fff; }
.full { position:absolute; left:0; top:0; width:1920px; height:1080px; }
.cam { position:absolute; left:0; top:0; width:1920px; height:1080px; transform-origin:0 0; }
.cam video, .cam img.shot { position:absolute; left:0; top:0; width:1920px; height:1080px; object-fit:cover; }
"""

def cam_xy(fx, fy, s):
    x = 960 - fx * s; y = 540 - fy * s
    x = min(0, max(1920 - 1920 * s, x)); y = min(0, max(1080 - 1080 * s, y))
    return round(x, 1), round(y, 1)

def scene(cid, start, dur, css, html, js):
    path = f'film/compositions/{cid}.html'
    open(path, 'w').write(f"""<template>
<style>
{COMMON_CSS}
{css}
</style>
<div id="root" data-composition-id="{cid}" data-width="1920" data-height="1080">
{html}
</div>
<script>
const tl = gsap.timeline({{ paused: true }});
const cam = (sel, x, y, s, at, d = 0.5, ease = "power2.inOut") => tl.to(sel, {{ x, y, scale: s, duration: d, ease }}, at);
{js}
window.__timelines["{cid}"] = tl;
</script>
</template>
""")
    return dict(cid=cid, start=round(start, 3), dur=round(dur, 3))

hosts = []

# ---------- A. Hook: what people get today ----------
a0 = 0; a1 = st('s04') - 0.05
L = lambda t: round(t - a0, 3)
TM_ROWS = [("svchost.exe", "0.4%", "312.4 MB"), ("svchost.exe", "0.1%", "188.0 MB"), ("chrome.exe", "3.2%", "1,204.8 MB"), ("svchost.exe", "0.0%", "96.3 MB"),
           ("RuntimeBroker.exe", "0.2%", "64.1 MB"), ("svchost.exe", "0.3%", "151.7 MB"), ("chrome.exe", "1.1%", "842.2 MB"), ("dwm.exe", "1.8%", "121.5 MB"),
           ("svchost.exe", "0.0%", "44.9 MB"), ("MsMpEng.exe", "2.4%", "233.0 MB")]
tm_rows = ''.join(f'<div class="tr{" sv" if n=="svchost.exe" else ""}"><span>{n}</span><span>{c}</span><span>{m}</span></div>' for n, c, m in TM_ROWS)
hosts.append(scene('hook', a0, a1 - a0, """
#root { background:#22252e; }
.desk { position:absolute; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:44px; }
.spin { width:120px; height:120px; border-radius:50%; border:12px solid rgba(255,255,255,.16); border-top-color:#9aa3b5; }
.slow { font-size:96px; font-weight:900; letter-spacing:-.03em; display:flex; gap:26px; }
.slow span { opacity:0; }
.slow span:first-child { opacity:1; }
.tm { position:absolute; left:260px; top:110px; width:1400px; height:780px; background:#2b2f3a; border:1px solid #444a58; border-radius:10px; opacity:0; overflow:hidden; }
.tm .bar { height:56px; background:#343947; display:flex; align-items:center; padding:0 26px; font-size:30px; font-weight:700; color:#c9ced8; }
.tm .heads { display:grid; grid-template-columns:1fr 200px 260px; padding:20px 30px 10px; font-size:28px; font-weight:700; color:#9aa3b5; }
.tm .heads b { color:#fff; }
.tm .heads .hot { color:#e09a8c; font-size:44px; font-weight:900; display:block; }
.tm .tr { display:grid; grid-template-columns:1fr 200px 260px; padding:12px 30px; font-size:30px; color:#d6dbe6; border-top:1px solid #3b4152; }
.tm .tr.sv span:first-child { color:#fff; }
.why { position:absolute; left:0; right:0; top:420px; text-align:center; font-size:110px; font-weight:900; color:#fff; opacity:0; text-shadow:0 6px 30px rgba(0,0,0,.6); }
.dl { position:absolute; left:50%; top:400px; width:1100px; margin-left:-550px; background:#2f333e; border:1px solid #444a58; border-radius:14px; padding:40px 48px; font-size:42px; opacity:0; }
.dl b { color:#d9b56a; }
.dl .track { margin-top:26px; height:20px; background:#444a58; border-radius:6px; overflow:hidden; }
.dl .fill { width:100%; height:100%; background:#d9b56a; transform-origin:0 50%; transform:scaleX(0); }
.cleaner { position:absolute; inset:0; background:radial-gradient(circle at 50% 40%, #a5503f 0%, #7a3329 55%, #3e1d19 100%); opacity:0; }
.brand { position:absolute; top:70px; left:0; right:0; text-align:center; font-size:40px; font-weight:900; letter-spacing:.06em; color:#e3c47a; }
.gauge { position:absolute; left:0; right:0; top:200px; text-align:center; }
.gauge .num { font-size:260px; font-weight:900; line-height:1; color:#fff; text-shadow:0 8px 0 #4a1d17; }
.gauge .lbl { font-size:72px; font-weight:900; color:#e3c47a; margin-top:10px; }
.boost { position:absolute; left:50%; top:640px; width:760px; margin-left:-380px; height:150px; border-radius:75px; background:#8db87a; color:#16220f; font-size:84px; font-weight:900; display:flex; align-items:center; justify-content:center; opacity:0; box-shadow:0 8px 0 #5d7d4f; }
.slower { position:absolute; left:0; right:0; top:820px; text-align:center; font-size:64px; font-weight:900; color:#fff; opacity:0; }
.slower em { font-style:normal; color:#e3c47a; }
.stamp { position:absolute; font-size:120px; font-weight:900; color:#fff; border:12px solid #fff; padding:6px 34px; border-radius:16px; opacity:0; letter-spacing:.02em; }
.price { position:absolute; right:150px; top:120px; background:#e3c47a; color:#3e1d19; font-weight:900; font-size:66px; padding:20px 36px; border-radius:14px; opacity:0; }
.price small { display:block; font-size:30px; }
.term { position:absolute; inset:0; background:#16181f; opacity:0; }
.term .win { position:absolute; left:210px; top:60px; width:1500px; height:750px; background:#0f1115; border:1px solid #3b4152; border-radius:14px; overflow:hidden; }
.term .bar { height:56px; background:#22252e; display:flex; align-items:center; justify-content:space-between; padding:0 26px; font-size:28px; font-weight:700; color:#c9ced8; }
.term .clock { font-family:'JetBrains Mono', monospace; color:#e3c47a; font-size:30px; }
.term pre { margin:0; padding:22px 30px; font-family:'JetBrains Mono', monospace; font-size:24px; line-height:1.45; color:#c9ced8; white-space:pre; }
.term .cmd { color:#8db87a; }
.term .ln { opacity:0; }
.term .tag { position:absolute; left:0; right:0; top:832px; text-align:center; font-size:46px; font-weight:800; color:#e3c47a; opacity:0; }
.lost { position:absolute; inset:0; display:flex; align-items:center; justify-content:center; font-size:84px; font-weight:900; opacity:0; background:#22252e; }
.lost span { color:#9aa3b5; }
.black { position:absolute; inset:0; background:#000; opacity:0; }
""", f"""
<div class="desk"><div class="spin" id="h-spin"></div>
  <div class="slow"><span>Your</span><span>PC</span><span>is</span><span>slow.</span></div></div>
<div class="tm" id="h-tm" data-layout-allow-overlap><div class="bar">Task Manager</div>
  <div class="heads"><span><b>Name</b></span><span>CPU<span class="hot">34%</span></span><span>Memory<span class="hot" id="h-mem">92%</span></span></div>
  {tm_rows}</div>
<div class="why" id="h-why" data-layout-allow-overlap>…but <em style="font-style:normal;color:#e3c47a">why?</em></div>
<div class="dl" id="h-dl" data-layout-allow-overlap>Downloading <b>PC_Booster_Ultra_Setup.exe</b><div class="track"><div class="fill" id="h-fill"></div></div></div>
<div class="cleaner" id="h-cleaner" data-layout-allow-overlap>
  <div class="brand">⚡ ULTRA PC BOOSTER PRO ⚡</div>
  <div class="gauge" id="h-gauge"><div class="num" id="h-num">0</div><div class="lbl">PROBLEMS FOUND!</div></div>
  <div class="boost" id="h-boost">BOOST RAM</div>
  <div class="slower" id="h-slower">Your PC is <em>87% slower!</em></div>
  <div class="stamp" id="h-st1" style="left:330px;top:300px" data-layout-allow-overlap>MEANS NOTHING</div>
  <div class="stamp" id="h-st2" style="left:620px;top:600px" data-layout-allow-overlap>MEANS NOTHING</div>
  <div class="stamp" id="h-st3" style="left:520px;top:770px;font-size:90px" data-layout-allow-overlap>MEANS NOTHING</div>
  <div class="price" id="h-price" data-layout-allow-overlap>PRO VERSION<br>$29.99<small>per year, auto-renew</small></div>
</div>
<div class="term" id="h-term" data-layout-allow-overlap><div class="win">
  <div class="bar"><span>Terminal: AI agent session</span><span class="clock" id="h-clock">00:00</span></div>
  <pre><span class="ln cmd" id="h-l0">PS C:\\> Get-Process | Sort-Object WS -Descending | Select-Object -First 15</span>\n<span class="ln" id="h-l1">Handles  NPM(K)    PM(K)     WS(K)   CPU(s)     Id ProcessName</span>\n<span class="ln" id="h-l2">-------  ------    -----     -----   ------     -- -----------</span>\n<span class="ln" id="h-l3">   2214     118   612448    498212   312.45   4000 chrome</span>\n<span class="ln" id="h-l4">   1876      96   402112    352104   201.13   4412 chrome</span>\n<span class="ln" id="h-l5">    988      61   210980    190240    88.02   5120 svchost</span>\n<span class="ln cmd" id="h-l6">PS C:\\> Get-Counter '\\Memory\\Available MBytes' -SampleInterval 2 -MaxSamples 5</span>\n<span class="ln" id="h-l7">Timestamp                 CounterSamples</span>\n<span class="ln" id="h-l8">30/10/2026 15:42:01       \\\\pc\\memory\\available mbytes :  1577</span>\n<span class="ln cmd" id="h-l9">PS C:\\> Get-ChildItem C:\\ -Recurse -ErrorAction SilentlyContinue | Measure-Object Length -Sum</span>\n<span class="ln" id="h-l10">  running...</span>\n</pre></div>
  <div class="tag" id="h-termtag">No tools built for the job.</div>
  <div class="stamp" id="h-st4" style="left:500px;top:420px;color:#e3c47a;border-color:#e3c47a" data-layout-allow-overlap>SLOW. UNCHECKED.</div>
</div>
<div class="lost" id="h-lost" data-layout-allow-overlap data-layout-allow-occlusion><div>…and you still don't know <span>what's wrong.</span></div></div>
<div class="black" id="h-black"></div>
""", f"""
tl.fromTo("#h-spin", {{ rotation: 0 }}, {{ rotation: 900, duration: {L(word('s01','open'))}, ease: "none" }}, 0);
["Your","PC","is","slow."].forEach((w, i) => i > 0 && tl.fromTo(`.slow span:nth-child(${{i+1}})`, {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.15, ease: "power4.out" }}, [0,{L(word('s01','PC'))},{L(word('s01','is'))},{L(word('s01','slow'))}][i]));
tl.to(".desk", {{ opacity: 0, duration: 0.2 }}, {L(word('s01','Task')-0.1)});
tl.fromTo("#h-tm", {{ opacity: 0, scale: 0.94 }}, {{ opacity: 1, scale: 1, duration: 0.35, ease: "expo.out" }}, {L(word('s01','Task')-0.1)});
tl.fromTo("#h-mem", {{ scale: 1 }}, {{ scale: 1.35, duration: 0.2, yoyo: true, repeat: 1, ease: "power2.out" }}, {L(word('s01b','ninety-two'))});
tl.fromTo(".tm .tr.sv", {{ backgroundColor: "rgba(224,154,140,0)" }}, {{ backgroundColor: "rgba(224,154,140,0.22)", duration: 0.12, stagger: 0.08 }}, {L(word('s01b','Thirty-one'))});
tl.fromTo("#h-why", {{ opacity: 0, scale: 1.2 }}, {{ opacity: 1, scale: 1, duration: 0.2, ease: "power4.out" }}, {L(word('s01b','never'))});
tl.to("#h-tm, #h-why", {{ opacity: 0, duration: 0.18 }}, {L(st('s01c')-0.1)});
tl.fromTo("#h-dl", {{ opacity: 0, y: 40 }}, {{ opacity: 1, y: 0, duration: 0.3, ease: "expo.out" }}, {L(word('s01c','download')-0.1)});
tl.fromTo("#h-fill", {{ scaleX: 0 }}, {{ scaleX: 1, duration: 0.9, ease: "power1.in" }}, {L(word('s01c','download'))});
tl.to("#h-dl", {{ opacity: 0, duration: 0.05 }}, {L(st('s02')-0.05)});
tl.fromTo("#h-cleaner", {{ opacity: 0, scale: 1.15 }}, {{ opacity: 1, scale: 1, duration: 0.18, ease: "power4.out" }}, {L(st('s02')-0.05)});
const n = {{ v: 0 }};
tl.fromTo(n, {{ v: 0 }}, {{ v: 1248, duration: 1.3, ease: "power2.out", onUpdate: () => {{ document.getElementById("h-num").textContent = Math.round(n.v).toLocaleString("en-US"); }} }}, {L(st('s02'))});
tl.fromTo("#h-boost", {{ opacity: 0, scale: 1.6 }}, {{ opacity: 1, scale: 1, duration: 0.25, ease: "back.out(2)" }}, {L(word('s02','Boost'))});
tl.to("#h-boost", {{ scale: 1.08, duration: 0.12, yoyo: true, repeat: 3, ease: "power1.inOut" }}, {L(word('s02','RAM')+0.1)});
tl.fromTo("#h-num", {{ scale: 1 }}, {{ scale: 1.06, duration: 0.15, yoyo: true, repeat: 1 }}, {L(word('s02','eighty-seven'))});
tl.fromTo("#h-slower", {{ opacity: 0, y: 30 }}, {{ opacity: 1, y: 0, duration: 0.2, ease: "expo.out" }}, {L(word('s02','Your',0))});
tl.to("#h-gauge, #h-boost, #h-slower", {{ filter: "saturate(0.3) blur(2px)", opacity: 0.55, duration: 0.3 }}, {L(word('s03','What'))});
[1,2,3].forEach((k, i) => tl.fromTo(`#h-st${{k}}`, {{ opacity: 0, scale: 2.2, rotation: [-8,6,-4][i] }}, {{ opacity: 1, scale: 1, rotation: [-8,6,-4][i], duration: 0.14, ease: "power4.in" }}, {L(word('s03','Nothing'))} + i * 0.18));
tl.fromTo("#h-price", {{ opacity: 0, scale: 1.8, rotation: 6 }}, {{ opacity: 1, scale: 1, rotation: 6, duration: 0.18, ease: "power4.in" }}, {L(word('s03','Real'))});
tl.set("#h-cleaner", {{ opacity: 0 }}, {L(st('s03b')-0.06)});
tl.fromTo("#h-term", {{ opacity: 0, scale: 1.04 }}, {{ opacity: 1, scale: 1, duration: 0.14, ease: "power4.out" }}, {L(st('s03b')-0.05)});
const lt = [{L(st('s03b')+0.35)}, {L(word('s03b','commands')-0.1)}, {L(word('s03b','commands')-0.05)}, {L(word('s03b','commands')+0.1)}, {L(word('s03b','commands')+0.2)}, {L(word('s03b','commands')+0.3)},
            {L(word('s03b','improvises'))}, {L(word('s03b','improvises')+0.5)}, {L(word('s03b','improvises')+0.6)}, {L(word('s03b','waits'))}, {L(word('s03b','raw'))}];
lt.forEach((at, k) => tl.fromTo(`#h-l${{k}}`, {{ opacity: 0 }}, {{ opacity: 1, duration: 0.05 }}, at));
const clk = {{ v: 0 }};
tl.fromTo(clk, {{ v: 8 }}, {{ v: 227, duration: {round(en('s03b')-st('s03b'),3)}, ease: "power1.in", onUpdate: () => {{ const s = Math.round(clk.v); document.getElementById("h-clock").textContent = String(Math.floor(s / 60)).padStart(2, "0") + ":" + String(s % 60).padStart(2, "0"); }} }}, {L(st('s03b'))});
tl.fromTo("#h-termtag", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.2 }}, {L(word('s03b','tools'))});
tl.fromTo("#h-st4", {{ opacity: 0, scale: 2.2, rotation: -6 }}, {{ opacity: 1, scale: 1, rotation: -6, duration: 0.14, ease: "power4.in" }}, {L(word('s03b','nothing'))});
tl.set("#h-term", {{ opacity: 0 }}, {L(st('s03c')-0.06)});
tl.fromTo("#h-lost", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.2, ease: "power2.out" }}, {L(st('s03c')-0.05)});
tl.fromTo("#h-black", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.15 }}, {L(en('s03c')+0.05)});
"""))

# ---------- B. Reveal ----------
b0 = st('s04') - 0.05; b1 = st('s05') - 0.05
L = lambda t: round(t - b0, 3)
hosts.append(scene('reveal', b0, b1 - b0, """
#root { background:#f2f3f5; color:#1f2430; }
.wrap { position:absolute; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:18px; }
#r-nox { width:300px; height:auto; }
.word { font-size:150px; font-weight:900; letter-spacing:-.045em; display:flex; }
.word span { display:inline-block; }
.sub { font-size:54px; font-weight:700; color:#5b74b8; }
""", """
<div class="wrap"><img id="r-nox" src="assets/nox/nox-hello.svg" alt="">
<div class="word" id="r-word"><span>P</span><span>C</span><span>&nbsp;</span><span>A</span><span>g</span><span>e</span><span>n</span><span>t</span></div>
<div class="sub" id="r-sub">does the opposite.</div></div>
""", f"""
tl.fromTo("#r-nox", {{ scale: 0.4, y: 80, opacity: 0 }}, {{ scale: 1, y: 0, opacity: 1, duration: 0.55, ease: "back.out(1.7)" }}, 0);
tl.fromTo("#r-word span", {{ opacity: 0.0, filter: "blur(6px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.12, stagger: 0.045, ease: "power4.out" }}, {L(word('s04','PC'))});
tl.fromTo("#r-sub", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.18, ease: "power4.out" }}, {L(word('s04','does'))});
"""))

# ---------- C. Ask: question, verified answer, sources ----------
c0 = st('s05') - 0.05; c1 = st('s08') - 0.05
L = lambda t: round(t - c0, 3)
seg1_end = st('s06') - 0.03
seg2_end = st('s07') - 0.03
m1 = M('chat', 'home') - 0.2
r1 = round((M('chat', 'answer') + 0.3 - m1) / (seg1_end - c0), 3)
m3 = M('chat', 'cite') - (word('s07', 'click') - seg2_end)
xs0, ys0 = cam_xy(1180, 470, 1.35)
xs1, ys1 = cam_xy(1170, 330, 1.5)
xc, yc = cam_xy(1182, 430, 1.8)
xc2, yc2 = cam_xy(1182, 450, 1.5)
xq, yq = cam_xy(1080, 330, 1.45)
xb, yb = cam_xy(1660, 150, 1.7)
CHAT_SEND = c0 + (M('chat', 'sent') - m1) / r1
hosts.append(scene('ask', c0, c1 - c0, """
#root { background:#fff; }
.tag { position:absolute; left:80px; top:70px; padding:14px 26px; border-radius:12px; background:#1f2430; color:#fff; font-size:38px; font-weight:800; opacity:0; z-index:5; }
.tag b { color:#8db87a; }
""", f"""
<div class="full" id="c-s1">
  <div class="cam" id="c-cam1"><video id="c-v1" class="clip" src="assets/clips/chat.mp4" muted playsinline data-start="0" data-duration="{round(seg1_end-c0,3)}" data-media-start="{round(m1,3)}" data-playback-rate="{r1}"></video></div>
</div>
<div class="full clip" id="c-s2" data-start="{L(seg1_end)}" data-duration="{round(seg2_end-seg1_end,3)}">
  <div class="cam" id="c-cam2"><img class="shot" src="assets/img/chat-answer.png" alt=""></div>
</div>
<div class="full" id="c-s3">
  <div class="cam" id="c-cam3"><video id="c-v3" class="clip" src="assets/clips/chat.mp4" muted playsinline data-start="{L(seg2_end)}" data-duration="{round(c1-seg2_end,3)}" data-media-start="{round(m3,3)}"></video></div>
</div>
<div class="tag" id="c-tag1">Nemotron <b>investigates</b> your PC</div>
<div class="tag" id="c-tag2">Every number → <b>its source</b></div>
""", f"""
tl.fromTo("#c-cam1", {{ x: {xs0}, y: {ys0}, scale: 1.35 }}, {{ x: {xs0}, y: {ys0}, scale: 1.35, duration: 0.01 }}, 0);
cam("#c-cam1", {xs1}, {ys1}, 1.5, {L(CHAT_SEND+0.25)}, 0.6);
tl.fromTo("#c-tag1", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.18 }}, {L(word('s05','investigates'))});
tl.to("#c-tag1", {{ opacity: 0, duration: 0.15 }}, {L(seg1_end-0.2)});
tl.fromTo("#c-cam2", {{ x: 0, y: 0, scale: 1.0 }}, {{ x: {xc}, y: {yc}, scale: 1.8, duration: 0.55, ease: "power3.inOut" }}, {L(word('s06','Chrome')-0.35)});
cam("#c-cam2", {xc2}, {yc2}, 1.5, {L(word('s06','real'))}, 0.6);
tl.fromTo("#c-cam3", {{ x: {xq}, y: {yq}, scale: 1.45 }}, {{ x: {xq}, y: {yq}, scale: 1.45, duration: 0.01 }}, {L(seg2_end)});
cam("#c-cam3", {xb}, {yb}, 1.7, {L(word('s07','Checked')-0.3)}, 0.55, "power3.inOut");
tl.fromTo("#c-tag2", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.18 }}, {L(word('s07','Every'))});
tl.to("#c-tag2", {{ opacity: 0, duration: 0.15 }}, {L(word('s07','Checked')-0.3)});
"""))

# ---------- D. Action you approve ----------
d0 = st('s08') - 0.05; d1 = st('s08b') - 0.05
L = lambda t: round(t - d0, 3)
T_REVIEW = word('s08', 'items'); T_RUN = word('s08', 'click')
dr = round((M('action', 'run') - M('action', 'review')) / (T_RUN - T_REVIEW), 3)
md = M('action', 'review') - (T_REVIEW - d0) * dr
xa, ya = cam_xy(1180, 560, 1.35)
xd, yd = cam_xy(960, 520, 1.6)
xu, yu = cam_xy(960, 570, 1.6)
xr, yr = cam_xy(1150, 650, 1.5)
hosts.append(scene('action', d0, d1 - d0, """
#root { background:#fff; }
""", f"""
<div class="cam" id="d-cam"><video id="d-v" class="clip" src="assets/clips/action.mp4" muted playsinline data-start="0" data-duration="{round(d1-d0,3)}" data-media-start="{round(md,3)}" data-playback-rate="{dr}"></video></div>
""", f"""
tl.fromTo("#d-cam", {{ x: {xa}, y: {ya}, scale: 1.35 }}, {{ x: {xa}, y: {ya}, scale: 1.35, duration: 0.01 }}, 0);
cam("#d-cam", {xd}, {yd}, 1.6, {L(T_REVIEW+0.15)}, 0.5, "power3.inOut");
cam("#d-cam", {xu}, {yu}, 1.6, {L(word('s08','how')-0.2)}, 0.4);
cam("#d-cam", {xr}, {yr}, 1.5, {L(word('s08','nothing')-0.3)}, 0.5);
cam("#d-cam", {xa}, {ya}, 1.35, {L(T_RUN+0.35)}, 0.45);
"""))

# ---------- D2. Storage scan ----------
s0 = st('s08b') - 0.05; s1 = st('s09') - 0.05
L = lambda t: round(t - s0, 3)
T_SEL = word('s08b', 'See')
ms1 = M('storage', 'scan') - 0.15
sr1 = round((M('storage', 'select') - 0.25 - ms1) / (T_SEL - 0.25 - s0), 3)
xs1_, ys1_ = cam_xy(960, 440, 1.2)
xt, yt = cam_xy(600, 420, 1.35)
xi, yi = cam_xy(1030, 560, 1.15)
hosts.append(scene('storage', s0, s1 - s0, """
#root { background:#fff; }
""", f"""
<div class="full" id="s-s1">
  <div class="cam" id="s-cam1"><video id="s-v1" class="clip" src="assets/clips/storage.mp4" muted playsinline data-start="0" data-duration="{L(T_SEL-0.25)}" data-media-start="{round(ms1,3)}" data-playback-rate="{sr1}"></video></div>
</div>
<div class="full" id="s-s2">
  <div class="cam" id="s-cam2"><video id="s-v2" class="clip" src="assets/clips/storage.mp4" muted playsinline data-start="{L(T_SEL-0.25)}" data-duration="{round(s1-(T_SEL-0.25),3)}" data-media-start="{round(M('storage','select')-0.25,3)}"></video></div>
</div>
""", f"""
tl.fromTo("#s-cam1", {{ x: 0, y: 0, scale: 1 }}, {{ x: {xs1_}, y: {ys1_}, scale: 1.2, duration: 0.5, ease: "power2.inOut" }}, 0.05);
tl.fromTo("#s-cam2", {{ x: {xt}, y: {yt}, scale: 1.35 }}, {{ x: {xt}, y: {yt}, scale: 1.35, duration: 0.01 }}, {L(T_SEL-0.25)});
cam("#s-cam2", {xi}, {yi}, 1.15, {L(word('s08b','then')-0.2)}, 0.55);
"""))

# ---------- E. Privacy ----------
e0 = st('s09') - 0.05; e1 = st('s10') - 0.05
L = lambda t: round(t - e0, 3)
# The dialog is on screen from 3.47 s to 6.87 s of the clip; the names chip is clicked at its mark.
PV_OPEN, PV_SHUT = 3.33, 6.83
ECUT = word('s09', 'files') + 0.5           # hold the dialog through "No files."
er = round((PV_SHUT - PV_OPEN) / (ECUT - e0), 3)
T_NAMES = word('s09', 'Your', 1) + 0.1
me2 = M('privacy', 'names') - (T_NAMES - ECUT)
xe, ye = cam_xy(961, 560, 1.45)
xn, yn = cam_xy(1100, 560, 1.75)
xc, yc = cam_xy(1000, 620, 1.7)
hosts.append(scene('privacy', e0, e1 - e0, """
#root { background:#fff; }
""", f"""
<div class="full" id="e-s1">
  <div class="cam" id="e-cam1"><video id="e-v1" class="clip" src="assets/clips/privacy.mp4" muted playsinline data-start="0" data-duration="{L(ECUT)}" data-media-start="{PV_OPEN}" data-playback-rate="{er}"></video></div>
</div>
<div class="full" id="e-s2">
  <div class="cam" id="e-cam2"><video id="e-v2" class="clip" src="assets/clips/privacy.mp4" muted playsinline data-start="{L(ECUT)}" data-duration="{round(e1-ECUT,3)}" data-media-start="{round(me2,3)}"></video></div>
</div>
""", f"""
tl.fromTo("#e-cam1", {{ x: 0, y: 0, scale: 1 }}, {{ x: {xe}, y: {ye}, scale: 1.45, duration: 0.45, ease: "power3.inOut" }}, 0.05);
cam("#e-cam1", {xn}, {yn}, 1.75, {L(word('s09','No',0)-0.15)}, 0.5);
tl.fromTo("#e-cam2", {{ x: {xc}, y: {yc}, scale: 1.7 }}, {{ x: {xc}, y: {yc}, scale: 1.7, duration: 0.01 }}, {L(ECUT)});
"""))

# ---------- F. Live PC, watches, daily report ----------
f0 = st('s10') - 0.05; f1 = st('s10b') - 0.05
L = lambda t: round(t - f0, 3)
fa = word('s10', 'PC', 1) - 0.1   # "PC Agent warns"
fb = word('s10', 'and', 0) - 0.05
T_SELECT = word('s10', 'Select'); T_WATCH = word('s10', 'watch') + 0.1
lr = round((M('live', 'monitor') - M('live', 'select')) / (T_WATCH - T_SELECT), 3)
mlive = M('live', 'select') - (T_SELECT - f0) * lr
xm, ym = cam_xy(620, 400, 1.45)
xf, yf = cam_xy(965, 597, 1.6)
hosts.append(scene('live', f0, f1 - f0, """
#root { background:#161615; }
""", f"""
<div class="full" id="f-s1">
  <div class="cam" id="f-cam1"><video id="f-v1" class="clip" src="assets/clips/live.mp4" muted playsinline data-start="0" data-duration="{L(fa)}" data-media-start="{round(max(0.05, mlive),3)}" data-playback-rate="{lr}"></video></div>
</div>
<div class="full" id="f-s2">
  <div class="cam" id="f-cam2"><video id="f-v2" class="clip" src="assets/clips/watches.mp4" muted playsinline data-start="{L(fa)}" data-duration="{round(fb-fa,3)}" data-media-start="{round(M('watches','top')-0.4,3)}"></video></div>
</div>
<div class="full" id="f-s3">
  <div class="cam" id="f-cam3"><video id="f-v3" class="clip" src="assets/clips/report.mp4" muted playsinline data-start="{L(fb)}" data-duration="{round(f1-fb,3)}" data-media-start="{round(M('report','report')-0.6,3)}"></video></div>
</div>
""", f"""
tl.fromTo("#f-cam1", {{ x: {cam_xy(900,480,1.3)[0]}, y: {cam_xy(900,480,1.3)[1]}, scale: 1.3 }}, {{ x: {cam_xy(900,480,1.3)[0]}, y: {cam_xy(900,480,1.3)[1]}, scale: 1.3, duration: 0.01 }}, 0);
cam("#f-cam1", {xm}, {ym}, 1.45, {L(T_SELECT-0.35)}, 0.5);
cam("#f-cam1", {xf}, {yf}, 1.6, {L(T_WATCH+0.15)}, 0.5);
tl.fromTo("#f-cam2", {{ x: 0, y: 0, scale: 1.12 }}, {{ x: {cam_xy(700,500,1.3)[0]}, y: {cam_xy(700,500,1.3)[1]}, scale: 1.3, duration: {round(fb-fa,3)}, ease: "power1.inOut" }}, {L(fa)});
tl.fromTo("#f-cam3", {{ x: 0, y: 0, scale: 1 }}, {{ x: {cam_xy(1180,760,1.5)[0]}, y: {cam_xy(1180,760,1.5)[1]}, scale: 1.5, duration: 0.6, ease: "power3.inOut" }}, {L(word('s10','daily')-0.25)});
"""))

# ---------- F2. History and search ----------
y0 = st('s10b') - 0.05; y1 = st('s11') - 0.05
L = lambda t: round(t - y0, 3)
ys_ = word('s10b', 'And') - 0.1
mh = M('history', 'charts') - 0.4
hr = round((M('history', 'end') - mh) / (ys_ - y0), 3)
hosts.append(scene('history', y0, y1 - y0, """
#root { background:#161615; }
""", f"""
<div class="full" id="y-s1">
  <div class="cam" id="y-cam1"><video id="y-v1" class="clip" src="assets/clips/history.mp4" muted playsinline data-start="0" data-duration="{L(ys_)}" data-media-start="{round(mh,3)}" data-playback-rate="{hr}"></video></div>
</div>
<div class="full" id="y-s2">
  <div class="cam" id="y-cam2"><video id="y-v2" class="clip" src="assets/clips/search.mp4" muted playsinline data-start="{L(ys_)}" data-duration="{round(y1-ys_,3)}" data-media-start="{round(M('search','typed')-0.35,3)}"></video></div>
</div>
""", f"""
tl.fromTo("#y-cam1", {{ x: 0, y: 0, scale: 1.1 }}, {{ x: {cam_xy(1100,520,1.3)[0]}, y: {cam_xy(1100,520,1.3)[1]}, scale: 1.3, duration: {L(ys_)}, ease: "power1.inOut" }}, 0);
tl.fromTo("#y-cam2", {{ x: {cam_xy(300,320,1.6)[0]}, y: {cam_xy(300,320,1.6)[1]}, scale: 1.6 }}, {{ x: {cam_xy(300,320,1.6)[0]}, y: {cam_xy(300,320,1.6)[1]}, scale: 1.6, duration: 0.01 }}, {L(ys_)});
"""))

# ---------- G. Web lookups with Tavily ----------
g0 = st('s11') - 0.05; g1 = st('s11b') - 0.05
L = lambda t: round(t - g0, 3)
gs = word('s11', 'Tavily') + 0.02
mw1 = M('web', 'sent') - 0.2
rate1 = round((M('web', 'answer') + 0.1 - mw1) / (gs - g0), 3)
xw, yw = cam_xy(1660, 830, 1.6)
xp, yp = cam_xy(926, 690, 1.45)
xst, yst = cam_xy(1170, 360, 1.45)
hosts.append(scene('web', g0, g1 - g0, """
#root { background:#fff; }
.tav { position:absolute; left:80px; top:70px; padding:14px 26px; border-radius:12px; background:#1f2430; color:#fff; font-size:38px; font-weight:800; opacity:0; z-index:5; }
.tav b { color:#b3a6d6; }
""", f"""
<div class="full" id="g-s1">
  <div class="cam" id="g-cam1"><video id="g-v1" class="clip" src="assets/clips/web.mp4" muted playsinline data-start="0" data-duration="{L(gs)}" data-media-start="{round(mw1,3)}" data-playback-rate="{rate1}"></video></div>
</div>
<div class="full" id="g-s2">
  <div class="cam" id="g-cam2"><video id="g-v2" class="clip" src="assets/clips/web.mp4" muted playsinline data-start="{L(gs)}" data-duration="{round(g1-gs,3)}" data-media-start="{round(M('web','answer')+0.1,3)}"></video></div>
</div>
<div class="tav" id="g-tag">Web lookups by <b>Tavily</b></div>
""", f"""
tl.fromTo("#g-cam1", {{ x: {xst}, y: {yst}, scale: 1.45 }}, {{ x: {xst}, y: {yst}, scale: 1.45, duration: 0.01 }}, 0);
tl.fromTo("#g-tag", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.18 }}, {L(gs)});
tl.fromTo("#g-cam2", {{ x: {xp}, y: {yp}, scale: 1.45 }}, {{ x: {xp}, y: {yp}, scale: 1.45, duration: 0.01 }}, {L(gs)});
cam("#g-cam2", {xw}, {yw}, 1.6, {L(word('s11','shows')-0.3)}, 0.55, "power3.inOut");
tl.to("#g-tag", {{ opacity: 0, duration: 0.15 }}, {L(word('s11','shows')-0.3)});
"""))

# ---------- G2. Memory upgrade price ----------
r0 = st('s11b') - 0.05; r1 = st('s12') - 0.05
L = lambda t: round(t - r0, 3)
rs = word('s11b', 'exact') - 0.1
mr1 = M('ram', 'sent') - 0.2
rr1 = round((M('ram', 'answer') + 0.1 - mr1) / (rs - r0), 3)
xra, yra = cam_xy(1182, 430, 1.45)
xrw, yrw = cam_xy(1188, 600, 1.33)
hosts.append(scene('ram', r0, r1 - r0, """
#root { background:#fff; }
.tav { position:absolute; left:80px; top:70px; padding:14px 26px; border-radius:12px; background:#1f2430; color:#fff; font-size:38px; font-weight:800; opacity:0; z-index:5; }
.tav b { color:#b3a6d6; }
""", f"""
<div class="full" id="r-s1">
  <div class="cam" id="r-cam1"><video id="r-v1" class="clip" src="assets/clips/ram.mp4" muted playsinline data-start="0" data-duration="{L(rs)}" data-media-start="{round(mr1,3)}" data-playback-rate="{rr1}"></video></div>
</div>
<div class="full" id="r-s2">
  <div class="cam" id="r-cam2"><video id="r-v2" class="clip" src="assets/clips/ram.mp4" muted playsinline data-start="{L(rs)}" data-duration="{round(r1-rs,3)}" data-media-start="{round(M('ram','answer')+0.1,3)}"></video></div>
</div>
<div class="tav" id="r-tag">Upgrade prices by <b>Tavily</b></div>
""", f"""
tl.fromTo("#r-cam1", {{ x: {cam_xy(1170,360,1.45)[0]}, y: {cam_xy(1170,360,1.45)[1]}, scale: 1.45 }}, {{ x: {cam_xy(1170,360,1.45)[0]}, y: {cam_xy(1170,360,1.45)[1]}, scale: 1.45, duration: 0.01 }}, 0);
tl.fromTo("#r-tag", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.15 }}, 0.1);
tl.fromTo("#r-cam2", {{ x: {xra}, y: {yra}, scale: 1.45 }}, {{ x: {xra}, y: {yra}, scale: 1.45, duration: 0.01 }}, {L(rs)});
cam("#r-cam2", {xrw}, {yrw}, 1.33, {L(word('s11b','with')-0.3)}, 0.55, "power3.inOut");
tl.to("#r-tag", {{ opacity: 0, duration: 0.15 }}, {L(rs)});
"""))

# ---------- H. Under the hood ----------
h0 = st('s12') - 0.05; h1 = st('s12b') - 0.05
L = lambda t: round(t - h0, 3)
TOOLS = ["memory", "processes", "CPU", "disk latency", "GPU · NVML", "battery", "storage scan", "drivers", "updates", "startup apps", "services", "event log", "network", "temperatures", "page faults", "browser tabs", "power plan", "volumes", "history", "recordings", "watches", "file sizes", "app details", "web lookup"]
import math
chips = []
for i, name in enumerate(TOOLS):
    side, k = divmod(i, 12)
    col, row = divmod(k, 6)
    cx = [230, 490, 1430, 1690][side * 2 + col]; cy = 300 + row * 96
    chips.append(f'<div class="chip" id="h-c{i}" style="left:{round(cx-118)}px;top:{round(cy-28)}px">{name}</div>')
hosts.append(scene('tech', h0, h1 - h0, """
#root { background:#1f2430; }
.kick { position:absolute; left:0; right:0; top:84px; text-align:center; font-size:46px; font-weight:700; color:#a9b4cc; opacity:0; }
.core { position:absolute; left:660px; top:380px; width:600px; height:220px; border-radius:20px; background:#5b74b8; display:flex; flex-direction:column; align-items:center; justify-content:center; opacity:0; }
.core .m { font-size:58px; font-weight:900; letter-spacing:-.02em; }
.core .p { font-size:30px; font-weight:700; color:#d6dbe6; margin-top:6px; }
.guess { position:absolute; left:0; right:0; top:640px; text-align:center; font-size:64px; font-weight:900; opacity:0; }
.guess s { color:#d98c80; text-decoration-thickness:8px; }
.guess b { color:#8db87a; }
.chip { position:absolute; width:236px; height:56px; border-radius:10px; background:#2a3142; border:2px solid #3b4459; font-size:28px; font-weight:700; color:#e3e6ec; display:flex; align-items:center; justify-content:center; opacity:0; }
.ro { position:absolute; left:0; right:0; top:660px; text-align:center; opacity:0; }
.ro span { display:inline-block; padding:14px 26px; border-radius:12px; background:#8db87a; color:#16220f; font-size:36px; font-weight:900; }
.check { position:absolute; left:360px; right:360px; top:330px; height:420px; border-radius:20px; background:#fff; color:#1f2430; padding:50px 60px; opacity:0; }
.check .q { font-size:30px; font-weight:700; color:#55607f; }
.check .line { font-size:62px; font-weight:800; margin-top:26px; position:relative; }
.check .bad { color:#d93025; }
.check .strike { position:absolute; left:0; top:50%; height:8px; width:100%; background:#d93025; transform-origin:0 50%; transform:scaleX(0); }
.check .good { color:#2e7a4d; opacity:0; margin-top:16px; }
.check .stamp { position:absolute; right:50px; top:44px; font-size:30px; font-weight:900; padding:10px 18px; border-radius:10px; opacity:0; }
.check .s1 { background:#fde8e6; color:#b42318; }
.check .s2 { background:#e9f3ec; color:#2e7a4d; }
""", f"""
<div class="kick" id="h-kick">Under the hood</div>
<div class="core" id="h-core"><div class="m">NVIDIA Nemotron</div><div class="p">on Nebius Token Factory</div></div>
<div class="guess" id="h-guess">It doesn't <s>guess</s>. It <b>measures</b>.</div>
{''.join(chips)}
<div class="ro" id="h-ro"><span>🔒 Read-only: look, never touch</span></div>
<div class="check" id="h-check">
  <div class="q">Draft answer</div>
  <div class="line bad" id="h-bad">Chrome holds 3.1 GiB<div class="strike" id="h-strike"></div></div>
  <div class="line good" id="h-good">Chrome holds 2.31 GiB ✓</div>
  <div class="stamp s1" id="h-s1">Checker: no measurement says 3.1</div>
  <div class="stamp s2" id="h-s2">Matches measurement O1</div>
</div>
""", f"""
tl.fromTo("#h-kick", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.18 }}, 0.05);
tl.fromTo("#h-core", {{ opacity: 0, scale: 0.7 }}, {{ opacity: 1, scale: 1, duration: 0.45, ease: "back.out(1.6)" }}, {L(word('s12','Nemotron'))});
tl.fromTo("#h-guess", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.18 }}, {L(word('s12','guess')-0.2)});
tl.to("#h-guess", {{ opacity: 0, duration: 0.15 }}, {L(word('s12','Two')-0.1)});
tl.to("#h-kick", {{ opacity: 0, duration: 0.15 }}, {L(word('s12','Two')-0.1)});
tl.fromTo(".chip", {{ opacity: 0, scale: 0.4 }}, {{ opacity: 1, scale: 1, duration: 0.3, ease: "back.out(2)", stagger: 0.045 }}, {L(word('s12','Two'))});
tl.fromTo("#h-ro", {{ opacity: 0, y: 30 }}, {{ opacity: 1, y: 0, duration: 0.3, ease: "expo.out" }}, {L(word('s12','look',0)+0.3)});
tl.to(".chip, #h-ro, #h-core, #h-kick", {{ opacity: 0, filter: "blur(8px)", duration: 0.2 }}, {L(word('s12','If')-0.15)});
tl.fromTo("#h-check", {{ opacity: 0, y: 60 }}, {{ opacity: 1, y: 0, duration: 0.4, ease: "expo.out" }}, {L(word('s12','If'))});
tl.fromTo("#h-strike", {{ scaleX: 0 }}, {{ scaleX: 1, duration: 0.25, ease: "power3.out" }}, {L(word('s12','catches'))});
tl.fromTo("#h-s1", {{ opacity: 0, scale: 1.4 }}, {{ opacity: 1, scale: 1, duration: 0.18, ease: "power4.out" }}, {L(word('s12','checker'))});
tl.to("#h-s1", {{ opacity: 0, duration: 0.12 }}, {L(word('s12','corrects')-0.05)});
tl.fromTo("#h-good", {{ opacity: 0, x: -30 }}, {{ opacity: 1, x: 0, duration: 0.3, ease: "expo.out" }}, {L(word('s12','corrects'))});
tl.fromTo("#h-s2", {{ opacity: 0, scale: 1.4 }}, {{ opacity: 1, scale: 1, duration: 0.18, ease: "power4.out" }}, {L(word('s12','corrects')+0.1)});
"""))

# ---------- H2. Also: dictation, screenshots, no key ----------
x0 = st('s12b') - 0.05; x1 = st('s13') - 0.05
L = lambda t: round(t - x0, 3)
hosts.append(scene('extras', x0, x1 - x0, """
#root { background:#1f2430; }
.row { position:absolute; left:160px; right:160px; top:260px; display:flex; gap:48px; }
.tile { flex:1; height:520px; border-radius:20px; background:#2a3142; border:2px solid #3b4459; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:30px; opacity:0; text-align:center; padding:0 30px; }
.tile .t { font-size:46px; font-weight:900; color:#fff; }
.tile .s { font-size:32px; font-weight:700; color:#b9c0cf; }
.tile img { width:150px; }
""", f"""
<div class="row">
  <div class="tile" id="x-1"><svg viewBox="0 0 24 24" width="120" height="120" fill="none" stroke="#e3e6ec" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.5a3 3 0 0 1 3 3V12a3 3 0 0 1-6 0V6.5a3 3 0 0 1 3-3z"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0"/><path d="M12 18v2.5"/></svg><div class="t">Local dictation</div><div class="s">Audio never leaves your PC</div></div>
  <div class="tile" id="x-2"><svg viewBox="0 0 24 24" width="120" height="120" fill="none" stroke="#e3e6ec" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 4.5h16v15H4z"/><path d="M4 16l5-5 4 4 3-3 4 4"/><path d="M15.5 8h.01"/></svg><div class="t">Screenshots &amp; photos</div><div class="s">Read by Nemotron Super VL</div></div>
  <div class="tile" id="x-3"><img src="assets/nox/nox-local.svg" alt=""><div class="t">No key? Still works</div><div class="s">Local checks run on your PC</div></div>
</div>
""", f"""
tl.fromTo("#x-1", {{ opacity: 0, y: 50 }}, {{ opacity: 1, y: 0, duration: 0.4, ease: "expo.out" }}, {L(word('s12b','Dictate'))});
tl.fromTo("#x-2", {{ opacity: 0, y: 50 }}, {{ opacity: 1, y: 0, duration: 0.4, ease: "expo.out" }}, {L(word('s12b','Attach'))});
tl.fromTo("#x-3", {{ opacity: 0, y: 50 }}, {{ opacity: 1, y: 0, duration: 0.4, ease: "expo.out" }}, {L(word('s12b','No'))});
"""))

# ---------- I. The model we trained ----------
i0 = st('s13') - 0.05; i1 = st('s14') - 0.05
it = word('s13', 'It') - 0.1
im = word('s13', 'picks') - 0.1
ie = st('s13b') - 0.05
L = lambda t: round(t - i0, 3)
def grid(prefix, n_green):
    cells = []
    for k in range(60):
        r, c = divmod(k, 10)
        cells.append(f'<div class="cell" id="{prefix}{k}" style="left:{c*58}px;top:{r*58}px"></div>')
    return ''.join(cells)
hosts.append(scene('trained', i0, i1 - i0, """
#root { background:#1f2430; }
.t1 { position:absolute; left:0; right:0; top:150px; text-align:center; font-size:96px; font-weight:900; letter-spacing:-.03em; opacity:0; }
.t1 b { color:#8db87a; }
.card { position:absolute; left:440px; top:380px; width:1040px; padding:36px 40px; border-radius:20px; background:#fff; color:#1f2430; opacity:0; text-align:center; }
.card .n { font-size:58px; font-weight:900; }
.card .row { display:flex; gap:14px; justify-content:center; margin-top:22px; }
.card .row span { font-size:28px; font-weight:800; padding:10px 18px; border-radius:10px; background:#eceff4; color:#3e4f7a; opacity:0; }
.q { position:absolute; left:0; right:0; top:420px; text-align:center; font-size:110px; font-weight:900; letter-spacing:-.03em; opacity:0; }
.q b { color:#8db87a; }
.exam { position:absolute; inset:0; opacity:0; }
.exam .head { position:absolute; left:0; right:0; top:80px; text-align:center; font-size:52px; font-weight:800; color:#d6dbe6; }
.side { position:absolute; top:220px; width:640px; }
.side .lbl { font-size:40px; font-weight:800; margin-bottom:22px; display:flex; justify-content:space-between; align-items:baseline; }
.side .cnt { font-size:96px; font-weight:900; }
.gridbox { position:relative; width:580px; height:348px; }
.cell { position:absolute; width:48px; height:48px; border-radius:8px; background:#343c52; border:2px solid #5d6884; }
.cell.on { }
#i-left { left:220px; } #i-right { left:1060px; }
#i-left .cnt { color:#d98c80; } #i-right .cnt { color:#8db87a; }
.spt { position:absolute; left:80px; top:70px; padding:14px 26px; border-radius:12px; background:#1f2430; color:#fff; font-size:38px; font-weight:800; }
.spt b { color:#8db87a; }
.foot { position:absolute; left:0; right:0; top:860px; text-align:center; font-size:36px; font-weight:700; color:#b9c0cf; opacity:0; }
.foot b { color:#fff; }
""", f"""
<div class="t1" id="i-t1">We trained <b>our own model</b>.</div>
<div class="card" id="i-card"><div class="n">Nemotron Nano 4B, fine-tuned</div>
  <div class="row"><span>QLoRA</span><span>NVIDIA L40S GPU</span><span>Nebius AI Cloud</span></div></div>
<div class="q" id="i-q">What should we <b>measure next</b>?</div>
<div class="full" id="i-app" style="opacity:0">
  <div class="cam" id="i-cam1"><video id="i-v1" class="clip" src="assets/clips/spec.mp4" muted playsinline data-start="{L(it)}" data-duration="{round(im-it,3)}" data-media-start="{round(M('spec','card')-0.2,3)}"></video></div>
  <div class="cam" id="i-cam2"><video id="i-v2" class="clip" src="assets/clips/spec.mp4" muted playsinline data-start="{L(im)}" data-duration="{round(ie-im,3)}" data-media-start="{round(M('spec','sent')+0.3,3)}" data-playback-rate="{round((M('spec','answer')-M('spec','sent')-0.3)/(ie-im),3)}"></video></div>
  <div class="spt" id="i-spt">Our model, <b>running in the app</b></div>
</div>
<div class="exam" id="i-exam">
  <div class="head">60 PC problems it had never seen</div>
  <div class="side" id="i-left"><div class="lbl"><span>Original Nano</span><span class="cnt" id="i-c0">0/60</span></div><div class="gridbox">{grid('i-a', 0)}</div></div>
  <div class="side" id="i-right"><div class="lbl"><span>Our Nano</span><span class="cnt" id="i-c1">0/60</span></div><div class="gridbox">{grid('i-b', 52)}</div></div>
  <div class="foot" id="i-foot">Right next measurement, <b>in a format the app can safely run</b></div>
</div>
""", f"""
tl.fromTo("#i-t1", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.2 }}, {L(word('s13','trained')-0.1)});
tl.fromTo("#i-card", {{ opacity: 0, y: 50 }}, {{ opacity: 1, y: 0, duration: 0.45, ease: "expo.out" }}, {L(word('s13','small'))});
tl.fromTo("#i-card .row span", {{ opacity: 0, scale: 0.6 }}, {{ opacity: 1, scale: 1, duration: 0.25, ease: "back.out(2)", stagger: 0.18 }}, {L(word('s13','fine-tuned'))});
tl.to("#i-t1, #i-card", {{ opacity: 0, filter: "blur(8px)", duration: 0.2 }}, {L(word('s13','what')-0.2)});
tl.fromTo("#i-q", {{ opacity: 0, scale: 0.92 }}, {{ opacity: 1, scale: 1, duration: 0.35, ease: "expo.out" }}, {L(word('s13','what'))});
tl.to("#i-q", {{ opacity: 0, filter: "blur(8px)", duration: 0.2 }}, {L(it-0.15)});
tl.fromTo("#i-app", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.2 }}, {L(it)});
tl.fromTo("#i-cam1", {{ x: 0, y: 0, scale: 1 }}, {{ x: {cam_xy(830,840,1.45)[0]}, y: {cam_xy(830,840,1.45)[1]}, scale: 1.45, duration: 0.5, ease: "power3.inOut" }}, {L(it)});
tl.fromTo("#i-cam2", {{ x: {cam_xy(1182,400,1.6)[0]}, y: {cam_xy(1182,400,1.6)[1]}, scale: 1.6 }}, {{ x: {cam_xy(1182,400,1.6)[0]}, y: {cam_xy(1182,400,1.6)[1]}, scale: 1.6, duration: 0.01 }}, {L(im)});
tl.to("#i-app", {{ opacity: 0, duration: 0.12 }}, {L(ie-0.14)});
tl.fromTo("#i-exam", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.2 }}, {L(word('s13b','On'))});
tl.fromTo("#i-exam .cell", {{ opacity: 0, scale: 0.3 }}, {{ opacity: 1, scale: 1, duration: 0.2, ease: "back.out(2)", stagger: 0.012 }}, {L(word('s13b','sixty'))});
tl.to("#i-left .cell", {{ borderColor: "#d98c80", duration: 0.3 }}, {L(word('s13b','none'))});
const greens = Array.from({{ length: 60 }}, (_, k) => k).filter(k => ![7, 13, 22, 31, 38, 44, 51, 57].includes(k));
const g0 = {L(word('s13b','Ours'))}, g1 = {L(word('s13b','fifty-two')+0.5)};
greens.forEach((k, j) => tl.to(`#i-b${{k}}`, {{ backgroundColor: "#8db87a", borderColor: "#a9cc98", duration: 0.08 }}, g0 + (g1 - g0) * j / greens.length));
const cnt = {{ v: 0 }};
tl.fromTo(cnt, {{ v: 0 }}, {{ v: 52, duration: g1 - g0, ease: "none", onUpdate: () => {{ document.getElementById("i-c1").textContent = Math.round(cnt.v) + "/60"; }} }}, g0);
tl.set("#i-c0", {{ textContent: "0/60" }}, {L(word('s13b','none'))});
tl.fromTo("#i-c1", {{ scale: 1 }}, {{ scale: 1.25, duration: 0.15, yoyo: true, repeat: 1, ease: "power2.out" }}, g1);
tl.fromTo("#i-foot", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.25 }}, g0 + 0.2);
"""))

# ---------- J. Impact ----------
j0 = st('s14') - 0.05; j1 = st('s15') - 0.05
L = lambda t: round(t - j0, 3)
hosts.append(scene('impact', j0, j1 - j0, """
#root { background:#1f2430; }
.half { position:absolute; top:0; height:1080px; width:960px; overflow:hidden; }
#j-l { left:0; background:radial-gradient(circle at 50% 40%, #a5503f 0%, #7a3329 60%, #3e1d19 100%); }
#j-r { left:960px; background:#fff; }
#j-l .num { position:absolute; left:0; right:0; top:330px; text-align:center; font-size:200px; font-weight:900; }
#j-l .lbl { position:absolute; left:0; right:0; top:560px; text-align:center; font-size:56px; font-weight:900; color:#e3c47a; }
.cap { position:absolute; left:0; right:0; bottom:150px; text-align:center; font-size:46px; font-weight:800; }
#j-r img { position:absolute; left:-560px; top:-120px; width:2304px; height:1296px; transform-origin:0 0; }
#j-r .cap { color:#1f2430; background:rgba(255,255,255,.92); padding:14px 0; }
.stats { position:absolute; inset:0; display:flex; align-items:center; justify-content:center; gap:60px; opacity:0; }
.stat { background:#fff; color:#1f2430; border-radius:20px; padding:40px 60px; text-align:center; }
.stat .v { font-size:120px; font-weight:900; letter-spacing:-.03em; }
.stat .k { font-size:36px; font-weight:700; color:#55607f; }
.stat.g .v { color:#2e7a4d; }
""", """
<div class="half" id="j-l" data-layout-allow-overflow><div class="num">1,248</div><div class="lbl">PROBLEMS FOUND!</div><div class="cap">Scares you</div></div>
<div class="half" id="j-r" data-layout-allow-overflow><img id="j-img" src="assets/img/chat-sources.png" alt=""><div class="cap">Tells you the truth</div></div>
<div class="stats" id="j-stats"><div class="stat"><div class="v">~10 s</div><div class="k">per answer</div></div><div class="stat g"><div class="v">$0.0044</div><div class="k">Nemotron 3 Super, per answer</div></div></div>
""", f"""
tl.fromTo("#j-l", {{ x: -960 }}, {{ x: 0, duration: 0.45, ease: "expo.out" }}, 0);
tl.fromTo("#j-r", {{ x: 960 }}, {{ x: 0, duration: 0.45, ease: "expo.out" }}, 0.08);
tl.to("#j-l", {{ filter: "saturate(0.2)", opacity: 0.6, duration: 0.3 }}, {L(word('s14','We'))});
tl.to("#j-l", {{ x: -960, duration: 0.5, ease: "power3.in" }}, {L(word('s14','truth')-0.3)});
tl.to("#j-r", {{ x: -960, width: 1920, duration: 0.5, ease: "power3.in" }}, {L(word('s14','truth')-0.3)});
tl.to("#j-img", {{ x: 470, duration: 0.5, ease: "power3.in" }}, {L(word('s14','truth')-0.3)});
tl.fromTo("#j-stats", {{ opacity: 0, scale: 1.08 }}, {{ opacity: 1, scale: 1, duration: 0.3, ease: "expo.out" }}, {L(word('s14','In')-0.1)});
tl.fromTo("#j-r", {{ filter: "blur(0px)", opacity: 1 }}, {{ filter: "blur(14px)", opacity: 0.35, duration: 0.3 }}, {L(word('s14','In')-0.1)});
"""))

# ---------- K. End card ----------
k0 = st('s15') - 0.05; k1 = TOTAL
L = lambda t: round(t - k0, 3)
hosts.append(scene('endcard', k0, k1 - k0, """
#root { background:#f2f3f5; color:#1f2430; }
#k-nox { position:absolute; left:810px; top:120px; width:300px; }
.name { position:absolute; left:0; right:0; top:410px; text-align:center; font-size:120px; font-weight:900; letter-spacing:-.045em; }
.lines { position:absolute; left:0; right:0; top:580px; display:flex; justify-content:center; gap:34px; font-size:64px; font-weight:900; }
.lines span { opacity:0; }
.lines .g { color:#2e7a4d; }
.foot { position:absolute; left:0; right:0; top:800px; text-align:center; font-size:34px; font-weight:700; color:#5b74b8; opacity:0; }
.foot2 { position:absolute; left:0; right:0; top:860px; text-align:center; font-size:30px; font-weight:700; color:#55607f; opacity:0; }
""", """
<img id="k-nox" src="assets/nox/nox-verified.svg" alt="">
<div class="name" id="k-name">PC Agent</div>
<div class="lines"><span id="k-l1">No boost buttons.</span><span id="k-l2">No bullshit.</span><span id="k-l3" class="g">Just proof.</span></div>
<div class="foot" id="k-f1">github.com/nassim-arifette/pc-agent</div>
<div class="foot2" id="k-f2">Built with NVIDIA Nemotron · Nebius Token Factory · Nebius AI Cloud · Tavily</div>
""", f"""
tl.fromTo("#k-nox", {{ opacity: 0, scale: 0.5, y: 60 }}, {{ opacity: 1, scale: 1, y: 0, duration: 0.5, ease: "back.out(1.7)" }}, 0);
tl.fromTo("#k-name", {{ opacity: 0, filter: "blur(10px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.2 }}, 0.1);
tl.fromTo("#k-l1", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.14 }}, {L(word('s15','No',0))});
tl.fromTo("#k-l2", {{ opacity: 0, filter: "blur(8px)" }}, {{ opacity: 1, filter: "blur(0px)", duration: 0.14 }}, {L(word('s15','No',1))});
tl.fromTo("#k-l3", {{ opacity: 0, scale: 1.3 }}, {{ opacity: 1, scale: 1, duration: 0.2, ease: "power4.out" }}, {L(word('s15','Just'))});
tl.fromTo("#k-f1, #k-f2", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.3, stagger: 0.12 }}, {L(word('s15','proof')+0.25)});
"""))

# ---------- Captions ----------
groups = []; cur = []
SKIP = {'s02', 's15'}
def cap(w): return w[:1].isupper() or w[:1].isdigit()
for w in W:
    if w['sid'] in SKIP: continue
    if cur:
        name_run = cap(cur[-1]['text']) and cap(w['text']) and cur[-1]['text'][-1] not in '.?!,:'
        if w['sid'] != cur[-1]['sid'] or w['start'] - cur[-1]['end'] > 0.35 or (len(cur) >= 5 and not name_run) or len(cur) >= 7:
            groups.append(cur); cur = []
    cur.append(w)
    if w['text'][-1] in '.?!,:' and len(cur) >= 2:
        groups.append(cur); cur = []
if cur: groups.append(cur)
merged = []
for g in groups:
    if merged and len(g) == 1 and g[0]['sid'] == merged[-1][-1]['sid'] and len(merged[-1]) <= 5 and g[0]['start'] - merged[-1][-1]['end'] < 0.35:
        merged[-1] = merged[-1] + g
    else:
        merged.append(g)
groups = merged
cap_html = []; cap_js = []
for gi, g in enumerate(groups):
    s0 = g[0]['start'] - 0.05
    nxt = groups[gi + 1][0]['start'] - 0.05 if gi + 1 < len(groups) else g[-1]['end'] + 0.4
    e0 = min(g[-1]['end'] + 0.35, nxt)
    txt = ' '.join(x['text'] for x in g).replace('&', '&amp;')
    cap_html.append(f'<div class="cg" id="cg{gi}"><span>{txt}</span></div>')
    cap_js.append(f'tl.fromTo("#cg{gi}", {{ opacity: 0 }}, {{ opacity: 1, duration: 0.08 }}, {round(s0,3)}); tl.set("#cg{gi}", {{ opacity: 0, visibility: "hidden" }}, {round(e0,3)});')
open('film/compositions/captions.html', 'w').write(f"""<template>
<style>
#root {{ position:absolute; inset:0; font-family: Inter, sans-serif; pointer-events:none; }}
.cg {{ position:absolute; left:0; right:0; bottom:64px; text-align:center; opacity:0; }}
.cg span {{ display:inline-block; font-size:44px; font-weight:700; color:#fff; background:rgba(14,18,48,.82); padding:10px 22px; border-radius:10px; }}
</style>
<div id="root" data-composition-id="captions" data-width="1920" data-height="1080">
{''.join(cap_html)}
</div>
<script>
const tl = gsap.timeline({{ paused: true }});
{chr(10).join(cap_js)}
window.__timelines["captions"] = tl;
</script>
</template>
""")

# ---------- Root ----------
host_html = []
for i, h in enumerate(hosts):
    host_html.append(f'<div id="{h["cid"]}" data-composition-id="{h["cid"]}" data-composition-src="compositions/{h["cid"]}.html" data-start="{h["start"]}" data-duration="{h["dur"]}" data-track-index="1" data-track-kind="graphics" data-width="1920" data-height="1080" style="z-index:{i+1}"></div>')
audio = []
for s in D['sched']:
    audio.append(f'<audio id="vo-{s["sid"]}" class="clip" src="assets/vo/{s["sid"]}.mp3" data-start="{s["start"]}" data-duration="{s["dur"]}" data-volume="1" data-track-index="3"></audio>')
music_start = st('s04') - 0.05
audio.append(f'<audio id="music" class="clip" src="assets/sfx/music.mp3" data-start="{round(music_start,3)}" data-duration="{round(min(TOTAL-music_start,134.9),3)}" data-volume="0.16" data-fade-in="0.2" data-fade-out="2.5" data-track-index="4"></audio>')
sfx = [('click', word('s01', 'Task') - 0.1, 0.4), ('alarm', st('s02') - 0.02, 0.2),
       ('stamp', word('s03', 'Nothing') + 0.13, 0.8), ('stamp', word('s03', 'Nothing') + 0.31, 0.7), ('stamp', word('s03', 'Nothing') + 0.49, 0.8), ('stamp', word('s03', 'Real') + 0.17, 0.6),
       ('stamp', word('s03b', 'nothing') + 0.13, 0.6),
       ('whoosh', st('s04') - 0.1, 0.45), ('click', CHAT_SEND, 0.5), ('whoosh', st('s06') - 0.15, 0.3),
       ('click', word('s07', 'click'), 0.5), ('ding', word('s07', 'Checked'), 0.3), ('whoosh', st('s08') - 0.15, 0.3), ('click', T_REVIEW, 0.5), ('click', T_RUN, 0.5),
       ('whoosh', st('s08b') - 0.15, 0.3), ('click', T_SEL, 0.45), ('whoosh', st('s09') - 0.15, 0.25), ('click', T_NAMES, 0.45), ('whoosh', st('s10') - 0.15, 0.3),
       ('click', T_SELECT, 0.45), ('click', T_WATCH, 0.45), ('whoosh', st('s10b') - 0.15, 0.3),
       ('whoosh', st('s11') - 0.15, 0.25), ('whoosh', st('s11b') - 0.15, 0.25), ('whoosh', st('s12') - 0.15, 0.35), ('buzz', word('s12', 'catches'), 0.4), ('ding', word('s12', 'corrects') + 0.1, 0.3),
       ('whoosh', st('s12b') - 0.15, 0.3), ('whoosh', st('s13') - 0.15, 0.3), ('tick', word('s13b', 'Ours'), 0.25), ('ding', word('s13b', 'fifty-two') + 0.5, 0.35),
       ('whoosh', st('s14') - 0.1, 0.35)]
import subprocess
SD = {n: float(subprocess.check_output(['ffprobe','-v','error','-show_entries','format=duration','-of','csv=p=0',f'film/assets/sfx/{n}.mp3'])) for n in set(x[0] for x in sfx)}
for i, (n, t, v) in enumerate(sfx):
    audio.append(f'<audio id="sfx{i}-{n}" class="clip" src="assets/sfx/{n}.mp3" data-start="{round(max(0,t),3)}" data-duration="{round(SD[n]-0.02,3)}" data-volume="{v}" data-track-index="{5 + i % 3}"></audio>')
open('film/index.html', 'w').write(f"""<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=1920, height=1080" />
<script src="https://cdn.jsdelivr.net/npm/gsap@3.14.2/dist/gsap.min.js"></script>
<style>
* {{ margin:0; padding:0; box-sizing:border-box; }}
html, body {{ margin:0; width:1920px; height:1080px; overflow:hidden; background:#000; }}
</style>
</head>
<body>
<div id="main" data-composition-id="main" data-start="0" data-duration="{TOTAL}" data-width="1920" data-height="1080">
{chr(10).join(host_html)}
<div id="captions" data-composition-id="captions" data-composition-src="compositions/captions.html" data-start="0" data-duration="{TOTAL}" data-track-index="2" data-track-kind="captions" data-width="1920" data-height="1080" style="z-index:50"></div>
{chr(10).join(audio)}
</div>
<script>
window.__timelines = window.__timelines || {{}};
window.__timelines["main"] = gsap.timeline({{ paused: true }});
</script>
</body>
</html>
""")
print('ok', len(hosts), 'scenes', len(groups), 'caption groups', TOTAL)
