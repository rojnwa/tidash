# /// script
# dependencies = ["httpx"]
# ///
"""S2: fetch the init + first N media segments of a TIDAL DASH stream, concatenated, into
spikes/s1-auth/.secrets/ (git-ignored). Uses the S1 session file; never prints the token or
segment URLs (they carry signed tokens). Delete the output after probing.

uv run --with httpx spikes/s2-audio/fetch_segments.py <track_id> [quality=HI_RES_LOSSLESS] [n=3]
"""
import base64, json, struct, sys, xml.etree.ElementTree as ET
from pathlib import Path
import httpx

SEC = Path(__file__).resolve().parent.parent / "s1-auth" / ".secrets"
NS = {"d": "urn:mpeg:dash:schema:mpd:2011"}


def atoms(b):
    i, out = 0, []
    while i + 8 <= len(b):
        n, t = struct.unpack(">I4s", b[i:i + 8]); out.append(t.decode()); i += n or len(b)
    return out


tid = sys.argv[1]
q = sys.argv[2] if len(sys.argv) > 2 else "HI_RES_LOSSLESS"
n = int(sys.argv[3]) if len(sys.argv) > 3 else 3
s = json.loads((SEC / "android.json").read_text())
r = httpx.get(f"https://api.tidal.com/v1/tracks/{tid}/playbackinfo",
              headers={"Authorization": f"Bearer {s['access_token']}"}, timeout=30,
              params=dict(audioquality=q, playbackmode="STREAM", assetpresentation="FULL",
                          countryCode=s["user"]["countryCode"]))
print("playbackinfo HTTP", r.status_code)
if r.status_code != 200: sys.exit(r.text[:300])
j = r.json()
print({k: j.get(k) for k in ("audioQuality", "bitDepth", "sampleRate", "manifestMimeType")})

root = ET.fromstring(base64.b64decode(j["manifest"]))
rep, st = root.find(".//d:Representation", NS), root.find(".//d:SegmentTemplate", NS)
durs = []
for e in st.find("d:SegmentTimeline", NS):
    durs += [int(e.get("d"))] * (int(e.get("r", 0)) + 1)
print("mpd: duration", root.get("mediaPresentationDuration"), "| rep", rep.get("id"), rep.get("codecs"),
      "| timescale", st.get("timescale"), "| startNumber", st.get("startNumber"), "| segments", len(durs),
      "| first durations", durs[:n], "| last", durs[-1], "| total", sum(durs))
print(f"expected frames in first {n} segments (timescale = sample rate):", sum(durs[:n]))

start = int(st.get("startNumber", 1))
urls = [st.get("initialization")] + [st.get("media").replace("$Number$", str(start + i)) for i in range(n)]
assert all(u.startswith("https://") for u in urls), "relative segment URLs (BaseURL) not handled"
parts = []
with httpx.Client(timeout=60) as c:
    for i, u in enumerate(urls):
        resp = c.get(u)
        if resp.status_code != 200: sys.exit(f"part {i} HTTP {resp.status_code}")
        parts.append(resp.content)
        print("init" if i == 0 else f"seg {start + i - 1}", len(resp.content), "bytes, atoms", atoms(resp.content))
out = SEC / f"s2_{tid}_init+{n}.mp4"
out.write_bytes(b"".join(parts))
print("wrote", out, out.stat().st_size, "bytes")
