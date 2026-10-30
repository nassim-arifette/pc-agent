import json, os, urllib.request, sys
key=os.environ["ELEVENLABS_API_KEY"]
items={
 "whoosh":("fast clean cinematic whoosh transition, short",0.8),
 "stamp":("heavy rubber stamp slam on paper, punchy, single hit",0.6),
 "alarm":("cheap software warning alarm beep, two fast beeps, annoying",1.0),
 "ding":("soft modern UI success chime, bright, single note",0.7),
 "click":("crisp mouse click, single",0.5),
 "buzz":("short UI error buzz, low, digital",0.5),
 "riser":("short tension riser into a hit, cinematic, modern",2.0),
 "tick":("rapid digital counter ticking, UI, short",1.5),
}
for name,(text,dur) in items.items():
    out=f"sfx/{name}.mp3"
    if os.path.exists(out): continue
    body={"text":text,"duration_seconds":dur,"prompt_influence":0.6}
    req=urllib.request.Request("https://api.elevenlabs.io/v1/sound-generation",data=json.dumps(body).encode(),headers={"xi-api-key":key,"Content-Type":"application/json"})
    try:
        open(out,"wb").write(urllib.request.urlopen(req).read()); print(name,"ok")
    except Exception as e: print(name,"ERR",e, getattr(e,'read',lambda:b'')()[:200])
