# Plazma Burst 2 — authenticated map scraper

Logs into <https://plazmaburst2.com/> with a real session, scrapes one custom-map
page, prints the five requested fields, and exports the page with all of its
dependencies for offline viewing.

Target page: `https://plazmaburst2.com/?s=9&id=5` (map **arena** by *yoshi96*).

---

## Setup

Requires Python 3.11+ (developed on 3.13.3).

```bash
python -m venv .venv
# Windows
.venv\Scripts\activate
# Linux/macOS
source .venv/bin/activate

pip install -r requirements.txt
```

Credentials live in `.env` (never hardcoded, git-ignored):

```ini
PB2_LOGIN=benchmark
PB2_PASSWORD=77e34116
```

A `.env.example` template is provided. Optional overrides, all with defaults:
`PB2_BASE_URL`, `PB2_MAP_ID`, `PB2_OUTPUT_DIR`, `PB2_TIMEOUT`,
`PB2_CONNECT_TIMEOUT`, `PB2_MAX_ATTEMPTS`, `PB2_BACKOFF`, `PB2_MIN_INTERVAL`.

## Run

```bash
python pb2_map_scraper.py                 # scrapes map 5 -> ./output
python pb2_map_scraper.py --map-id 1 -v   # different map, debug logging
python pb2_map_scraper.py --output-dir ./run-2026-09-23
```

### Terminal output

```
====================================================================
 Plazma Burst 2 - authenticated map scrape
====================================================================
 Source          : https://plazmaburst2.com/?s=9&id=5
 Page ID         : 5
 Authenticated as: benchmark (uid 1902385)
 Scraped at      : 2026-09-23T00:13:20+00:00
--------------------------------------------------------------------
 Map name        : arena
 Map ID          : yoshi96-arena
 Votes           : 3
 Map description : New map by yoshi96.
 Map designer    : yoshi96
--------------------------------------------------------------------
 Exports:
   output\map_5.json  (443 bytes)
   ...
```

### Exports (`output/`, UTF-8)

| File | Contents |
| --- | --- |
| `map_5.json` | JSON **array** with the five fields plus provenance (`source_url`, `map_page_id`, `scraped_at`, `authenticated_as`, `authenticated_uid`, `designer_profile_url`) |
| `map_5_page.html` | Byte-verbatim copy of the authenticated response |
| `map_5_offline.html` | Same page rewritten to load from `assets/` — renders offline with no network |
| `assets/*` | Stylesheets, scripts, images, fonts; CSS `url(...)` references are rewritten to local filenames |

`map_5_offline.html` was verified in Chrome with DNS blackholed: 90 CSS rules
loaded, header graphics/fonts/map preview all local, authenticated navigation
("Welcome back, benchmark!") rendered, zero network requests.

---

## How the login works (verified against the live site)

The login is a plain HTML form POST — no CAPTCHA on the HTTP path, so no browser
is needed.

1. **`GET /`** — returns the `log_form` form (`action=""`, so it posts to the same
   URL) and a per-page token: `var ses="3cabbd71…"`.
2. **Client-side password hashing** — the page's `upd()` handler (from
   `md5-min.js`) sets a *hidden* field to `hex_md5(password)`:
   `document.getElementById('pass2').value = hex_md5(document.getElementById('pass').value)`.
   The real `<input id="pass">` is **not** submitted. The scraper therefore posts
   `password = md5(password).hexdigest()`, the hex digest, not the plaintext.
3. **`POST /`** — form fields `login`, `password` (the digest), `Submit=Log-in`,
   with `Referer`/`Origin` set to the site. The server replies with cookies
   `login`, `password`, `pb2uid`.

### Login verification

An HTTP 200 is **not** proof of success — the site returns 200 for every outcome
below, and a *failed* login still sets `login`/`password`/`pb2uid` cookies:

| Outcome | Status | Cookies set | Body marker |
| --- | --- | --- | --- |
| Success | 200 | yes | `logout_for=<login>` link, authenticated nav |
| Wrong password | 200 | **yes** | `alert("Incorrect login or password entered…")`, `log_form` still present |
| Flood protection | 200 | no | `418 - Page will not be displayed` |
| Cloudflare challenge | 403 | no | `Just a moment...` |

So verification is **body-based**, and three conditions must all hold:

1. the failure alert `Incorrect login or password entered` is absent;
2. a `logout_for=<login>` link is present and its account name matches `PB2_LOGIN`;
3. the `pb2uid` cookie is present.

Then, before scraping, `assert_authenticated()` re-fetches the homepage and
requires the logout link again — so a session that silently expired between login
and scrape aborts instead of producing an anonymous page. The map page is
re-checked the same way: if it carries `log_form` without `logout_for`, the run
fails as an authentication error rather than parsing a logged-out page.

## How extraction works

The five fields are read from the map-details table, located structurally rather
than by fixed offsets:

1. Find `<td id="content_header_block">`; if its text contains `Map Access Error`,
   raise a parse/access error (invalid or unpublished maps return 200 too).
2. Find the table cell whose text is `Map name`, walk up to its `<table>`, and
   build a `label -> value` map from every row (`<td>label:</td> … <td>value</td>`),
   taking the last cell of each row as the value.
3. Read `Map name`, `Map ID`, `Votes`, `Map description`, `Map Designer`; `Votes`
   is normalised to an integer; `Map Designer` also yields the designer's profile
   link.
4. Any missing field aborts the run with the list of missing labels and the labels
   actually seen — a layout change fails loudly instead of exporting `null`s.

On the target page `Map ID` is the map's slug (`yoshi96-arena`); the numeric page
id from the URL is recorded separately as `map_page_id`.

## Reliability

* **Timeouts** — connect and read timeouts on every request (`PB2_CONNECT_TIMEOUT`,
  `PB2_TIMEOUT`).
* **Bounded retries** — transport errors, 429/5xx, Cloudflare challenges and the
  site's HTTP-200 flood-protection page are retried with exponential backoff plus
  jitter, capped by `PB2_MAX_ATTEMPTS`.
* **Retry-After** — honoured when the server sends it; the flood-protection body
  is detected by its text because the status code lies (200).
* **Rate limiting** — requests are serialised with a minimum 1.5 s gap
  (`PB2_MIN_INTERVAL`) and no concurrency; asset downloads are sequential too.
* **TLS** — verification stays on (`verify=True`); an `SSLError` raises
  `TlsVerificationError` and is never retried with verification disabled.
* **No clobbering** — exports are written to a staging directory and only moved
  into place with `os.replace` after login, fetch, parse *and* asset download all
  succeed. A failed run leaves previous exports byte-identical (verified).

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | configuration error |
| 2 | authentication failed or unproven |
| 3 | network / TLS / unexpected HTTP status |
| 4 | parse error or map not accessible |
| 5 | blocked by flood protection or Cloudflare challenge |
| 130 | interrupted |

## Notes and limits

* The site's own flood protection returns HTTP 200 with a `418` body after a
  burst of requests; the scraper backs off and reports it clearly instead of
  exporting a block page. Very frequent runs from one IP may need a longer
  `PB2_MIN_INTERVAL`.
* Unauthenticated deep links are answered by Cloudflare with a JS challenge
  (HTTP 403). The HTTP-only flow works because the homepage is not challenged and
  the authenticated session carries through; if a challenge ever appears mid-run,
  the tool reports it rather than trying to defeat it.
* No CAPTCHA is presented on this login path, so no CAPTCHA handling is needed.
  Should one appear, the run fails with an explicit message instead of guessing.
* Ads and third-party embeds (AdSense, CPMStar, Cloudflare beacon) are left as
  remote URLs in the offline copy and the Cloudflare beacon is removed, since it
  cannot work from disk.
* `.env` holds a live credential: it is git-ignored and never logged. Use
  `PB2_LOGIN`/`PB2_PASSWORD` from the environment in CI.
