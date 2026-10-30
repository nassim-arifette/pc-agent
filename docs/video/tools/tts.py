import json, os, base64, urllib.request, sys
key=os.environ["ELEVENLABS_API_KEY"]; voice=sys.argv[1] if len(sys.argv)>1 else "nPczCjzI2devNBz1zQrb"
lines=json.load(open("vo.json")); prev=None
for i,(sid,text) in enumerate(lines):
    out=f"vo/{sid}.mp3"
    if os.path.exists(out): prev=text; continue
    body={"text":text,"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.42,"similarity_boost":0.8,"style":0.35,"use_speaker_boost":True,"speed":1.05}}
    if prev: body["previous_text"]=prev
    if i+1<len(lines): body["next_text"]=lines[i+1][1]
    req=urllib.request.Request(f"https://api.elevenlabs.io/v1/text-to-speech/{voice}/with-timestamps?output_format=mp3_44100_192",data=json.dumps(body).encode(),headers={"xi-api-key":key,"Content-Type":"application/json"})
    r=json.load(urllib.request.urlopen(req))
    open(out,"wb").write(base64.b64decode(r["audio_base64"]))
    json.dump(r["alignment"],open(f"vo/{sid}.align.json","w"))
    prev=text; print(sid,"ok")
