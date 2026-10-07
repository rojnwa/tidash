# /// script
# dependencies = ["httpx"]
# ///
"""S1 throwaway TIDAL auth/manifest probe. No stdin prompts.

uv run --with httpx spikes/s1-auth/probe.py login-url <cand>
uv run --with httpx spikes/s1-auth/probe.py login-finish <cand> "<redirect_url>"
uv run --with httpx spikes/s1-auth/probe.py refresh <cand>
uv run --with httpx spikes/s1-auth/probe.py info <cand> <track_id>
uv run --with httpx spikes/s1-auth/probe.py fixtures <cand> <track_id> <album_id> [<bad_track_id>]

Client secret (python-tidal sends it only on refresh; set it to test that) come from env TIDAL_SECRET_<CAND>.
"""
import base64, hashlib, json, os, secrets, sys, time, urllib.parse, webbrowser
from pathlib import Path
import httpx

HERE = Path(__file__).resolve().parent
SEC = HERE / ".secrets"
FIX = HERE.parent.parent / "crates" / "tidal" / "tests" / "fixtures"
AUTH = "https://auth.tidal.com/v1/oauth2/token"
API = "https://api.tidal.com/v1"
# name -> (client_id, redirect_uri, appMode, needs_secret)
CANDS = {
    "web": ("49YxDN9a2aFV6RTG", "https://desktop.tidal.com/login/auth", "WEB", False),  # REJECTED: error 11102
    "web-listen": ("49YxDN9a2aFV6RTG", "https://listen.tidal.com/login/auth", "WEB", False),
    "desktop": ("mhPVJJEBNRzVjr2p", "tidal://login/auth", "DESKTOP", False),
    "android": ("6BDSRdpK9hqEBTgU", "https://tidal.com/android/login/auth", "android", False),
}
SCOPE = "r_usr w_usr"
QUALITIES = ["LOW", "HIGH", "LOSSLESS", "HI_RES_LOSSLESS"]


def b64(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()
def cand(n):
    if n not in CANDS: sys.exit(f"unknown candidate {n}; have {list(CANDS)}")
    return CANDS[n]
def secret(n): return os.environ.get(f"TIDAL_SECRET_{n.upper()}")
def tokfile(n): return SEC / f"{n}.json"
def load(n): return json.loads(tokfile(n).read_text())
def save(n, d):
    SEC.mkdir(exist_ok=True); tokfile(n).write_text(json.dumps(d, indent=2))


def login_url(n):
    cid, redir, mode, _ = cand(n)
    ver = b64(secrets.token_bytes(32)); chal = b64(hashlib.sha256(ver.encode()).digest())
    state = b64(secrets.token_bytes(12)); uniq = secrets.token_hex(8)
    q = dict(response_type="code", redirect_uri=redir, client_id=cid, lang="EN", appMode=mode,
             client_unique_key=uniq, code_challenge=chal, code_challenge_method="S256")
    if n == "android":  # python-tidal pkce_login_url, verbatim: no scope/state, restrict_signup
        q["restrict_signup"] = "true"; state = ""
    else: q.update(scope=SCOPE, state=state)
    SEC.mkdir(exist_ok=True)
    (SEC / f"{n}.pkce.json").write_text(json.dumps(dict(verifier=ver, state=state, unique=uniq)))
    url = "https://login.tidal.com/authorize?" + urllib.parse.urlencode(q, quote_via=urllib.parse.quote)
    print(url); webbrowser.open(url)


def post_token(n, form):
    cid, _, _, needs = cand(n)
    form = {"client_id": cid, **form}
    if needs or secret(n):
        s = secret(n) or sys.exit(f"set TIDAL_SECRET_{n.upper()}")
        form["client_secret"] = s
    r = httpx.post(AUTH, data=form, timeout=30)
    print("token HTTP", r.status_code)
    return r


def login_finish(n, redirect):
    cid, redir, _, _ = cand(n)
    pk = json.loads((SEC / f"{n}.pkce.json").read_text())
    qs = urllib.parse.parse_qs(urllib.parse.urlparse(redirect).query)
    if pk["state"] and qs.get("state", [pk["state"]])[0] != pk["state"]: sys.exit("state mismatch")
    if "code" not in qs: sys.exit(f"no code in redirect: {qs}")
    # python-tidal exchange: no client_secret, scope literally "r_usr+w_usr+w_sub"
    r = post_token(n, dict(grant_type="authorization_code", code=qs["code"], redirect_uri=redir,
                           code_verifier=pk["verifier"], client_unique_key=pk["unique"],
                           scope="r_usr+w_usr+w_sub" if n == "android" else SCOPE))
    d = r.json()
    if r.status_code != 200: sys.exit(json.dumps(d))
    d["_client_id"] = cid; d["_obtained"] = int(time.time()); d["_unique"] = pk["unique"]
    save(n, d)
    print("saved", tokfile(n), "keys:", sorted(d), "scope:", d.get("scope"), "user keys:", sorted(d.get("user", {})))


def refresh(n):
    d = load(n)
    # python-tidal refresh sends client_secret (no scope); here it is sent only if env TIDAL_SECRET_<CAND> is set
    r = post_token(n, dict(grant_type="refresh_token", refresh_token=d["refresh_token"]))
    nd = r.json()
    if r.status_code != 200: sys.exit(json.dumps(nd))
    print("refresh keys:", sorted(nd), "| new refresh token returned:", "refresh_token" in nd,
          "| user object:", "user" in nd)
    (SEC / f"{n}.refresh_raw.json").write_text(json.dumps(nd, indent=2))
    d.update(nd); d["_obtained"] = int(time.time()); save(n, d)


def hdr(n): return {"Authorization": f"Bearer {load(n)['access_token']}"}
def cc(n): return (load(n).get("user") or {}).get("countryCode") or "US"


def pbi(n, tid, ep, q):
    return httpx.get(f"{API}/tracks/{tid}/{ep}", headers=hdr(n), timeout=30, params=dict(
        audioquality=q, playbackmode="STREAM", assetpresentation="FULL", countryCode=cc(n)))


def info(n, tid):
    for ep in ("playbackinfo", "playbackinfopostpaywall"):
        for q in QUALITIES:
            r = pbi(n, tid, ep, q)
            if r.status_code != 200:
                print(f"{ep:24} {q:16} HTTP {r.status_code} {r.text[:200]}"); continue
            j = r.json()
            out = {k: j.get(k) for k in ("audioQuality", "audioMode", "manifestMimeType", "bitDepth", "sampleRate")}
            raw = base64.b64decode(j["manifest"])
            try: m = json.loads(raw)
            except Exception: m = None
            if m: out.update({k: m.get(k) for k in ("mimeType", "codecs", "encryptionType")})
            else:  # DASH MPD
                import re
                x = raw.decode(errors="replace")
                out.update(mpd_mime=re.search(r'mimeType="([^"]+)"', x[x.find("<AdaptationSet"):]).group(1),
                           codecs=re.findall(r'codecs="([^"]+)"', x), rep=re.findall(r'<Representation id="([^"]+)"', x),
                           encryption="ContentProtection" in x)
            print(f"{ep:24} {q:16}", json.dumps(out))


def fixtures(n, tid, album, bad=None):
    FIX.mkdir(parents=True, exist_ok=True)
    def w(name, r):
        (FIX / name).write_text(json.dumps(r.json(), indent=2)); print(name, r.status_code)
    c = cc(n); h = hdr(n)
    (FIX / "token.json").write_text(json.dumps(load(n), indent=2)); print("token.json")
    rf = SEC / f"{n}.refresh_raw.json"
    if rf.exists(): (FIX / "refresh.json").write_text(rf.read_text()); print("refresh.json")
    def g(p, **kw): return httpx.get(API + p, headers=h, params=dict(countryCode=c, **kw), timeout=30)
    w("search.json", g("/search", query="radiohead", limit=5, types="ARTISTS,ALBUMS,TRACKS"))
    w("album.json", g(f"/albums/{album}"))
    w("album_items_page1.json", g(f"/albums/{album}/items", limit=50, offset=0))
    w("album_items_page2.json", g(f"/albums/{album}/items", limit=50, offset=50))
    w("playbackinfo_lossless.json", pbi(n, tid, "playbackinfopostpaywall", "LOSSLESS"))
    w("playbackinfo_hires.json", pbi(n, tid, "playbackinfopostpaywall", "HI_RES_LOSSLESS"))
    if bad: w("playbackinfo_error_4005.json", pbi(n, bad, "playbackinfopostpaywall", "LOSSLESS"))
    print("NOW SANITIZE fixtures (Step 5)")


if __name__ == "__main__":
    a = sys.argv[1:]
    cmd = {"login-url": login_url, "login-finish": login_finish, "refresh": refresh, "info": info, "fixtures": fixtures}.get(a[0] if a else "")
    if not cmd: sys.exit(__doc__)
    cmd(*a[1:])
