#!/usr/bin/env python3
"""Authenticated scraper for Plazma Burst 2 custom-map pages.

Target: https://plazmaburst2.com/?s=9&id=5

The site's login is a plain HTML form POST (no CAPTCHA on the HTTP path):

  1. GET  /                     -> page carries ``var ses="<token>"`` and the
                                   ``log_form`` form (action="" -> same URL).
  2. POST /                     -> fields ``login``, ``password`` (the *MD5 hex*
                                   of the password, computed client-side by
                                   ``upd()`` from md5-min.js), ``Submit=Log-in``.
                                   The server replies with cookies ``login``,
                                   ``password`` and ``pb2uid``.

Gotchas this module defends against (all observed on the live site):

  * A successful login and a *failed* login both return HTTP 200, and a failed
    login still sets ``login``/``password``/``pb2uid`` cookies. Cookie presence
    alone is therefore NOT proof of authentication - the response body must
    contain the authenticated navigation (``logout_for=<login>``) and must NOT
    contain the ``Incorrect login or password entered`` alert.
  * The site's own flood protection returns HTTP 200 with a body saying
    ``418 - Page will not be displayed`` / ``too unusually big ammount of page
    requests``. That is a transient condition, not a page.
  * Cloudflare answers unauthenticated deep links with HTTP 403 and a
    ``Just a moment...`` JS challenge page.
  * Invalid or unpublished maps also return HTTP 200, with
    ``Map Access Error`` in the content block.

Exports are staged and only moved into place after login, fetch and parse have
all succeeded, so a failed run can never clobber a previous good export.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import random
import re
import shutil
import sys
import time
import urllib.parse
from dataclasses import dataclass, field
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
from typing import Any

import requests
from bs4 import BeautifulSoup
from dotenv import load_dotenv

# --------------------------------------------------------------------------- #
# Defined values (overridable through .env / environment / CLI)
# --------------------------------------------------------------------------- #

DEFAULT_BASE_URL = "https://plazmaburst2.com/"
DEFAULT_MAP_ID = "5"  # https://plazmaburst2.com/?s=9&id=5
DEFAULT_OUTPUT_DIR = "output"

DEFAULT_TIMEOUT = 30.0  # read timeout, seconds
DEFAULT_CONNECT_TIMEOUT = 10.0  # connect timeout, seconds
DEFAULT_MAX_ATTEMPTS = 4  # bounded retries per request
DEFAULT_BACKOFF = 5.0  # initial backoff, seconds
DEFAULT_MAX_BACKOFF = 60.0
DEFAULT_MIN_INTERVAL = 1.5  # polite minimum gap between requests, seconds

USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
)

# Page markers (verbatim strings taken from the live site).
LOGIN_FORM_MARKER = 'name="log_form"'
LOGIN_FAILURE_MARKER = "Incorrect login or password entered"
SESSION_TOKEN_RE = re.compile(r'var\s+ses\s*=\s*"([^"]*)"')
LOGOUT_LINK_RE = re.compile(r"logout_for=([^&\"']+)")
RATE_LIMIT_MARKERS = (
    "418 - Page will not be displayed",
    "too unusually big ammount of page requests",
)
CHALLENGE_MARKER = "just a moment"
MAP_ACCESS_ERROR_MARKER = "Map Access Error"

# The five fields the benchmark asks for, in display order.
# Maps a normalised page label -> JSON field name.
WANTED_FIELDS: tuple[tuple[str, str], ...] = (
    ("Map name", "map_name"),
    ("Map ID", "map_id"),
    ("Votes", "votes"),
    ("Map description", "map_description"),
    ("Map Designer", "map_designer"),
)

# Static extensions we keep as-is when naming downloaded assets.
STATIC_EXTENSIONS = {
    ".css", ".js", ".mjs", ".png", ".gif", ".jpg", ".jpeg", ".svg", ".ico",
    ".webp", ".bmp", ".otf", ".ttf", ".woff", ".woff2", ".mp3", ".ogg",
    ".wav", ".mp4", ".webm", ".json", ".txt", ".xml",
}
EXTENSION_BY_CONTENT_TYPE = {
    "text/css": ".css",
    "text/javascript": ".js",
    "application/javascript": ".js",
    "application/x-javascript": ".js",
    "image/gif": ".gif",
    "image/png": ".png",
    "image/jpeg": ".jpg",
    "image/svg+xml": ".svg",
    "image/webp": ".webp",
    "image/x-icon": ".ico",
    "image/vnd.microsoft.icon": ".ico",
    "font/otf": ".otf",
    "font/ttf": ".ttf",
    "font/woff": ".woff",
    "font/woff2": ".woff2",
    "application/font-woff": ".woff",
}

# Asset attributes rewritten for the offline copy.
ASSET_ATTRS = ("src", "href")
ASSET_TAGS = ("link", "script", "img", "source", "iframe", "embed", "input")
# Absolute URLs on these hosts are left alone (third-party, not needed offline).
SKIP_REMOTE_HOSTS = ("googlesyndication.com", "google-analytics.com",
                     "doubleclick.net", "cloudflare.com", "cloudflareinsights.com")

log = logging.getLogger("pb2")


# --------------------------------------------------------------------------- #
# Errors
# --------------------------------------------------------------------------- #


class ScrapeError(Exception):
    """Base class; ``exit_code`` is the process status for this failure."""

    exit_code = 1


class NetworkError(ScrapeError):
    """Transport failure or retries exhausted on a transient condition."""

    exit_code = 3


class TlsVerificationError(NetworkError):
    """TLS verification failed. Verification is intentionally never disabled."""

    exit_code = 3


class BlockedError(ScrapeError):
    """Flood protection or a Cloudflare challenge prevented the request."""

    exit_code = 5


class AuthenticationError(ScrapeError):
    """Login was rejected or could not be proven."""

    exit_code = 2


class HttpStatusError(ScrapeError):
    """Unexpected non-retryable HTTP status."""

    exit_code = 3


class ParseError(ScrapeError):
    """The page loaded but the expected data was not present."""

    exit_code = 4


class MapAccessError(ScrapeError):
    """The map page answered with ``Map Access Error`` (missing/unpublished)."""

    exit_code = 4


# --------------------------------------------------------------------------- #
# Settings
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class Settings:
    base_url: str
    map_id: str
    output_dir: Path
    login: str
    password: str
    timeout: float
    connect_timeout: float
    max_attempts: int
    backoff: float
    max_backoff: float
    min_interval: float

    @property
    def map_url(self) -> str:
        return f"{self.base_url}?s=9&id={urllib.parse.quote(str(self.map_id))}"


def _env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None or raw.strip() == "":
        return default
    try:
        return float(raw)
    except ValueError as exc:
        raise ScrapeError(f"{name} must be a number, got {raw!r}") from exc


def _env_int(name: str, default: int) -> int:
    return int(_env_float(name, float(default)))


def load_settings(args: argparse.Namespace, env_file: Path | None = None) -> Settings:
    """Build settings from CLI args, falling back to environment/.env."""
    env_path = env_file or Path(__file__).with_name(".env")
    if env_path.exists():
        load_dotenv(env_path, override=False)

    login = (args.login or os.getenv("PB2_LOGIN") or "").strip()
    password = args.password or os.getenv("PB2_PASSWORD") or ""
    if not login or not password:
        raise AuthenticationError(
            "Missing credentials. Set PB2_LOGIN and PB2_PASSWORD in .env "
            f"({env_path}) or pass --login/--password."
        )

    base_url = args.base_url or os.getenv("PB2_BASE_URL") or DEFAULT_BASE_URL
    if not base_url.endswith("/"):
        base_url += "/"

    return Settings(
        base_url=base_url,
        map_id=str(args.map_id or os.getenv("PB2_MAP_ID") or DEFAULT_MAP_ID),
        output_dir=Path(args.output_dir or os.getenv("PB2_OUTPUT_DIR") or DEFAULT_OUTPUT_DIR),
        login=login,
        password=password,
        timeout=_env_float("PB2_TIMEOUT", DEFAULT_TIMEOUT),
        connect_timeout=_env_float("PB2_CONNECT_TIMEOUT", DEFAULT_CONNECT_TIMEOUT),
        max_attempts=_env_int("PB2_MAX_ATTEMPTS", DEFAULT_MAX_ATTEMPTS),
        backoff=_env_float("PB2_BACKOFF", DEFAULT_BACKOFF),
        max_backoff=_env_float("PB2_MAX_BACKOFF", DEFAULT_MAX_BACKOFF),
        min_interval=_env_float("PB2_MIN_INTERVAL", DEFAULT_MIN_INTERVAL),
    )


# --------------------------------------------------------------------------- #
# HTTP client: timeouts, bounded retries, Retry-After, no TLS relaxation
# --------------------------------------------------------------------------- #


def _retry_after_seconds(response: requests.Response) -> float | None:
    raw = response.headers.get("Retry-After")
    if not raw:
        return None
    raw = raw.strip()
    if raw.isdigit():
        return float(raw)
    try:
        when = parsedate_to_datetime(raw)
    except (TypeError, ValueError):
        return None
    if when.tzinfo is None:
        when = when.replace(tzinfo=timezone.utc)
    return max(0.0, (when - datetime.now(timezone.utc)).total_seconds())


def _html_text(response: requests.Response, limit: int = 8000) -> str:
    ctype = response.headers.get("Content-Type", "").lower()
    if "html" not in ctype and "text" not in ctype:
        return ""
    return response.text[:limit]


def classify_block(response: requests.Response) -> str | None:
    """Return ``"challenge"``/``"rate_limited"`` when a block page was served."""
    if response.headers.get("cf-mitigated"):
        return "challenge"
    body = _html_text(response)
    if not body:
        return None
    low = body.lower()
    if CHALLENGE_MARKER in low:
        return "challenge"
    if any(marker.lower() in low for marker in RATE_LIMIT_MARKERS):
        return "rate_limited"
    return None


class HttpClient:
    """Sequential, throttled HTTP session with bounded retries.

    One connection pool, no concurrency: the target site actively throttles
    bursty clients, so requests are serialised with a minimum interval.
    """

    def __init__(self, settings: Settings) -> None:
        self.settings = settings
        self.session = requests.Session()
        self.session.headers.update(
            {
                "User-Agent": USER_AGENT,
                "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,"
                          "image/avif,image/webp,*/*;q=0.8",
                "Accept-Language": "en-US,en;q=0.9",
            }
        )
        self._last_request_at = 0.0
        self.request_count = 0

    # -- internals --------------------------------------------------------- #

    def _throttle(self) -> None:
        wait = self.settings.min_interval - (time.monotonic() - self._last_request_at)
        if wait > 0:
            time.sleep(wait)

    def _sleep(self, attempt: int, retry_after: float | None, reason: str) -> None:
        delay = retry_after if retry_after is not None else min(
            self.settings.max_backoff, self.settings.backoff * (2 ** (attempt - 1))
        )
        delay += random.uniform(0, min(1.5, delay * 0.1))  # jitter
        log.warning("transient: %s - retrying in %.1fs (attempt %d/%d)",
                    reason, delay, attempt, self.settings.max_attempts)
        time.sleep(delay)

    def request(self, method: str, url: str, **kwargs: Any) -> requests.Response:
        """Perform a request, retrying transient failures within bounds."""
        kwargs.setdefault("allow_redirects", True)
        kwargs.setdefault("verify", True)  # TLS verification stays ON, always.

        last_error: str = "unknown"
        for attempt in range(1, self.settings.max_attempts + 1):
            self._throttle()
            try:
                response = self.session.request(
                    method,
                    url,
                    timeout=(self.settings.connect_timeout, self.settings.timeout),
                    **kwargs,
                )
            except requests.exceptions.SSLError as exc:
                raise TlsVerificationError(
                    f"TLS verification failed for {url}: {exc}. Certificate "
                    "verification is required and is never disabled by this tool."
                ) from exc
            except requests.exceptions.RequestException as exc:
                last_error = f"{type(exc).__name__}: {exc}"
                self._last_request_at = time.monotonic()
                if attempt == self.settings.max_attempts:
                    raise NetworkError(
                        f"{method} {url} failed after {attempt} attempts: {last_error}"
                    ) from exc
                self._sleep(attempt, None, last_error)
                continue
            finally:
                self._last_request_at = time.monotonic()
                self.request_count += 1

            block = classify_block(response)
            if block == "challenge":
                if attempt == self.settings.max_attempts:
                    raise BlockedError(
                        "Cloudflare presented a JS challenge "
                        f"(HTTP {response.status_code}) for {url}. The HTTP-only "
                        "flow is sufficient for the login and map pages; if this "
                        "persists, the IP is being challenged and a real browser "
                        "session would be required."
                    )
                self._sleep(attempt, _retry_after_seconds(response), "cloudflare challenge")
                continue

            if block == "rate_limited":
                if attempt == self.settings.max_attempts:
                    raise BlockedError(
                        "The site's flood protection blocked this client "
                        f"(HTTP {response.status_code}, body reports 418) for {url}. "
                        "Wait a few minutes or raise PB2_MIN_INTERVAL."
                    )
                self._sleep(attempt, _retry_after_seconds(response),
                            "site flood protection (HTTP 200 block page)")
                continue

            if response.status_code == 429 or 500 <= response.status_code < 600:
                last_error = f"HTTP {response.status_code}"
                if attempt == self.settings.max_attempts:
                    raise NetworkError(f"{method} {url} kept returning {last_error}")
                self._sleep(attempt, _retry_after_seconds(response), last_error)
                continue

            if response.status_code >= 400:
                raise HttpStatusError(
                    f"{method} {url} returned HTTP {response.status_code} "
                    f"({response.headers.get('Content-Type', 'unknown type')})"
                )

            return response

        raise NetworkError(f"{method} {url} failed: {last_error}")

    def get(self, url: str, **kwargs: Any) -> requests.Response:
        return self.request("GET", url, **kwargs)

    def post(self, url: str, **kwargs: Any) -> requests.Response:
        return self.request("POST", url, **kwargs)

    def get_bytes(self, url: str, referer: str | None = None) -> tuple[bytes, str]:
        """Fetch a binary/asset resource. Returns ``(content, content_type)``."""
        headers = {"Referer": referer} if referer else None
        response = self.get(url, headers=headers)
        return response.content, response.headers.get("Content-Type", "")


# --------------------------------------------------------------------------- #
# Authentication
# --------------------------------------------------------------------------- #


@dataclass
class LoginResult:
    login: str
    uid: str | None
    session_token: str | None
    verified_by: str
    cookies: dict[str, str]


def login(client: HttpClient) -> LoginResult:
    """Log in and *prove* the session is authenticated before returning."""
    settings = client.settings

    # Step 1: GET the homepage - establishes the session and yields the
    # per-page ``ses`` token plus the login form definition.
    page = client.get(settings.base_url)
    html = page.text
    if LOGIN_FORM_MARKER not in html:
        raise AuthenticationError(
            "Login form not found on the homepage; the page structure changed "
            "or the response was not the expected site page."
        )
    token_match = SESSION_TOKEN_RE.search(html)
    session_token = token_match.group(1) if token_match else None
    log.info("loaded login form (session token %s)",
             f"{session_token[:8]}...{session_token[-4:]}" if session_token else "n/a")

    # Step 2: the browser hashes the password client-side (upd() -> hex_md5)
    # and posts the digest in the hidden ``password`` field.
    password_hash = hashlib.md5(settings.password.encode("utf-8")).hexdigest()
    response = client.post(
        settings.base_url,
        data={"login": settings.login, "password": password_hash, "Submit": "Log-in"},
        headers={
            "Referer": settings.base_url,
            "Origin": settings.base_url.rstrip("/"),
            "Content-Type": "application/x-www-form-urlencoded",
        },
    )
    body = response.text

    # Step 3: verify. HTTP 200 proves nothing here - both outcomes are 200 and
    # both set cookies, so the body decides.
    if LOGIN_FAILURE_MARKER in body:
        raise AuthenticationError(
            "The site rejected the credentials "
            f"('{LOGIN_FAILURE_MARKER}'). Check PB2_LOGIN / PB2_PASSWORD in .env."
        )

    cookies = {c.name: c.value for c in client.session.cookies}
    uid = cookies.get("pb2uid")
    logout_match = LOGOUT_LINK_RE.search(body)
    logout_user = logout_match.group(1) if logout_match else None

    if not logout_user or logout_user.lower() != settings.login.lower():
        raise AuthenticationError(
            "Login POST returned HTTP "
            f"{response.status_code} but the session is not authenticated "
            f"(no 'logout_for={settings.login}' link in the response; "
            f"logout link found: {logout_user!r}). The login form is still "
            "present, so the credentials or the account state were not accepted."
        )
    if not uid:
        raise AuthenticationError(
            "Authenticated navigation appeared but the 'pb2uid' cookie is "
            "missing; refusing to continue with an unverified session."
        )

    verified_by = f"logout_for={logout_user} + pb2uid cookie"
    log.info("login verified: %s (uid %s)", verified_by, uid)
    return LoginResult(
        login=settings.login,
        uid=uid,
        session_token=session_token,
        verified_by=verified_by,
        cookies=cookies,
    )


def assert_authenticated(client: HttpClient) -> None:
    """Re-check the session on a fresh page load before scraping."""
    html = client.get(client.settings.base_url).text
    if LOGIN_FAILURE_MARKER in html or "logout_for=" not in html:
        raise AuthenticationError(
            "Session is no longer authenticated (no logout link on the "
            "homepage); aborting before scraping."
        )


# --------------------------------------------------------------------------- #
# Scraping + parsing
# --------------------------------------------------------------------------- #


@dataclass
class MapRecord:
    map_name: str
    map_id: str
    votes: int
    map_description: str
    map_designer: str
    map_page_id: str
    source_url: str
    scraped_at: str
    authenticated_as: str
    authenticated_uid: str | None
    designer_profile_url: str | None = None
    extra_fields: dict[str, str] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        return {
            "map_name": self.map_name,
            "map_id": self.map_id,
            "votes": self.votes,
            "map_description": self.map_description,
            "map_designer": self.map_designer,
            "designer_profile_url": self.designer_profile_url,
            "map_page_id": self.map_page_id,
            "source_url": self.source_url,
            "scraped_at": self.scraped_at,
            "authenticated_as": self.authenticated_as,
            "authenticated_uid": self.authenticated_uid,
        }


def _clean(text: str) -> str:
    return re.sub(r"\s+", " ", text.replace("\xa0", " ")).strip()


def fetch_map_page(client: HttpClient) -> tuple[bytes, str]:
    """Fetch the authenticated map page. Returns ``(content, url)``."""
    url = client.settings.map_url
    response = client.get(url, headers={"Referer": client.settings.base_url})
    content = response.content
    text = response.text
    if LOGIN_FORM_MARKER in text and "logout_for=" not in text:
        raise AuthenticationError(
            f"The map page {url} was served as an anonymous visitor (login form "
            "present, no authenticated navigation); refusing to parse it."
        )
    log.info("fetched map page %s (%d bytes)", url, len(content))
    return content, url


def parse_map_page(content: bytes, url: str, map_page_id: str,
                   session: LoginResult) -> MapRecord:
    """Extract the five requested fields from the map page HTML.

    ``content`` is passed as raw bytes so BeautifulSoup can honour the document's
    declared charset (the site serves ``charset=UTF-8``).
    """
    soup = BeautifulSoup(content, "html.parser")

    header = soup.find(id="content_header_block")
    if header is None:
        raise ParseError(
            "Content block '#content_header_block' not found; the page layout "
            "changed or the response was not the map page."
        )
    header_text = _clean(header.get_text(" "))
    if MAP_ACCESS_ERROR_MARKER in header_text:
        raise MapAccessError(f"{MAP_ACCESS_ERROR_MARKER}: {header_text}")

    # The map details live in the table that owns the "Map name:" label row.
    label_cell = soup.find(
        "td", string=lambda s: s is not None and _clean(s).rstrip(":").lower() == "map name"
    )
    if label_cell is None:
        raise ParseError("Could not locate the map details table ('Map name:' row).")
    table = label_cell.find_parent("table")
    if table is None:
        raise ParseError("Map details table disappeared while parsing.")

    fields: dict[str, str] = {}
    for row in table.find_all("tr"):
        cells = row.find_all("td", recursive=False)
        if len(cells) < 2:
            continue
        label = _clean(cells[0].get_text(" ")).rstrip(":").strip()
        if not label:
            continue
        value_cell = cells[-1]
        fields.setdefault(label, _clean(value_cell.get_text(" ")))

    missing = [label for label, _ in WANTED_FIELDS if not fields.get(label)]
    if missing:
        raise ParseError(
            f"Missing expected map field(s) on {url}: {', '.join(missing)}. "
            f"Fields seen: {sorted(fields)}"
        )

    votes_raw = fields["Votes"]
    try:
        votes = int(re.sub(r"[^\d]", "", votes_raw) or "0")
    except ValueError as exc:
        raise ParseError(f"Could not parse the Votes value {votes_raw!r}") from exc

    designer_cell = None
    for row in table.find_all("tr"):
        cells = row.find_all("td", recursive=False)
        if len(cells) >= 2 and _clean(cells[0].get_text(" ")).rstrip(":").lower() == "map designer":
            designer_cell = cells[-1]
            break
    designer_link = designer_cell.find("a", href=True) if designer_cell else None
    designer_profile_url = (
        urllib.parse.urljoin(url, designer_link["href"]) if designer_link else None
    )

    return MapRecord(
        map_name=fields["Map name"],
        map_id=fields["Map ID"],
        votes=votes,
        map_description=fields["Map description"],
        map_designer=fields["Map Designer"],
        map_page_id=str(map_page_id),
        source_url=url,
        scraped_at=datetime.now(timezone.utc).isoformat(timespec="seconds"),
        authenticated_as=session.login,
        authenticated_uid=session.uid,
        designer_profile_url=designer_profile_url,
        extra_fields={
            k: v for k, v in fields.items()
            if k not in {label for label, _ in WANTED_FIELDS} and v
        },
    )


# --------------------------------------------------------------------------- #
# Offline export (page + its dependencies)
# --------------------------------------------------------------------------- #


class AssetDownloader:
    """Downloads page dependencies through the authenticated session."""

    def __init__(self, client: HttpClient, referer: str) -> None:
        self.client = client
        self.referer = referer
        self.assets: dict[str, bytes] = {}  # local filename -> content
        self.by_url: dict[str, str] = {}  # absolute URL -> local filename
        self.failures: list[str] = []

    def _filename(self, url: str, content_type: str) -> str:
        split = urllib.parse.urlsplit(url)
        base = os.path.basename(split.path) or "index"
        stem, ext = os.path.splitext(base)
        if ext.lower() not in STATIC_EXTENSIONS:
            ext = EXTENSION_BY_CONTENT_TYPE.get(content_type.split(";")[0].strip().lower(), ".bin")
        if split.query:
            stem = f"{stem}_{hashlib.sha1(split.query.encode('utf-8')).hexdigest()[:8]}"
        name = re.sub(r"[^A-Za-z0-9._-]", "_", f"{stem}{ext}")
        # De-duplicate names across distinct URLs.
        if name in self.assets and self.by_url.get(url) != name:
            name = f"{os.path.splitext(name)[0]}_{hashlib.sha1(url.encode()).hexdigest()[:6]}" \
                   f"{os.path.splitext(name)[1]}"
        return name

    def fetch(self, url: str) -> str | None:
        """Download ``url`` once; returns the local filename or None on failure."""
        if url in self.by_url:
            return self.by_url[url]
        try:
            content, content_type = self.client.get_bytes(url, referer=self.referer)
        except ScrapeError as exc:
            log.warning("asset failed: %s (%s)", url, exc)
            self.failures.append(url)
            return None
        name = self._filename(url, content_type)
        self.assets[name] = content
        self.by_url[url] = name
        log.debug("asset ok: %s -> %s (%d bytes)", url, name, len(content))
        return name

    def fetch_css_dependencies(self, css_url: str, css_text: str) -> str:
        """Download ``url(...)`` targets referenced by a stylesheet."""
        css_base = urllib.parse.urlsplit(css_url)
        for ref in set(re.findall(r"url\(\s*['\"]?([^'\")]+)['\"]?\s*\)", css_text)):
            ref = ref.strip()
            if not ref or ref.startswith(("data:", "#", "http:", "https:", "//")):
                continue
            absolute = urllib.parse.urljoin(css_url, ref)
            if urllib.parse.urlsplit(absolute).netloc != css_base.netloc:
                continue
            local = self.fetch(absolute)
            if local:
                css_text = css_text.replace(ref, local)
        return css_text


def build_offline_html(content: bytes, page_url: str, downloader: AssetDownloader) -> str:
    """Rewrite the page so it renders from ``assets/`` with no network."""
    soup = BeautifulSoup(content, "html.parser")
    origin_host = urllib.parse.urlsplit(page_url).netloc

    # 1. Stylesheets/scripts/images: download and point at the local copy.
    for tag in soup.find_all(ASSET_TAGS):
        for attr in ASSET_ATTRS:
            raw = tag.get(attr)
            if not raw or raw.startswith(("data:", "#", "javascript:", "mailto:")):
                continue
            absolute = urllib.parse.urljoin(page_url, raw)
            parts = urllib.parse.urlsplit(absolute)
            if parts.scheme not in ("http", "https"):
                continue
            if parts.netloc != origin_host:
                continue  # third-party: leave the absolute URL in place
            if any(host in parts.netloc for host in SKIP_REMOTE_HOSTS):
                continue
            if parts.path.startswith("/cdn-cgi/"):
                continue
            if tag.name == "a":
                # Navigation links cannot work offline; keep them usable online.
                tag[attr] = absolute
                continue
            local = downloader.fetch(absolute)
            if local is None:
                continue
            if local in downloader.assets and local.endswith(".css"):
                css_text = downloader.assets[local].decode("utf-8", errors="replace")
                rewritten = downloader.fetch_css_dependencies(absolute, css_text)
                if rewritten != css_text:
                    downloader.assets[local] = rewritten.encode("utf-8")
            tag[attr] = f"assets/{local}"

    # 2. Drop the Cloudflare beacon: an external script tag and the inline
    #    loader that injects it into a hidden iframe. Neither works offline.
    for tag in soup.find_all("script"):
        src = tag.get("src") or ""
        if "/cdn-cgi/" in src:
            tag.decompose()
        elif "__CF$cv$params" in (tag.string or ""):
            tag.decompose()

    return "<!DOCTYPE html>\n" + soup.decode()


def commit_exports(out_dir: Path, files: dict[str, bytes]) -> list[Path]:
    """Stage every file, then move it into place.

    Nothing is written to the output directory until the whole export has been
    assembled, so a failed run leaves previous exports untouched.
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    staging = out_dir / f".staging-{os.getpid()}-{int(time.time() * 1000)}"
    staging.mkdir()
    written: list[Path] = []
    try:
        for rel, data in files.items():
            dst = staging / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(data)
        for rel in files:
            src, dst = staging / rel, out_dir / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            os.replace(src, dst)  # atomic within the same volume
            written.append(dst)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    return written


# --------------------------------------------------------------------------- #
# Presentation
# --------------------------------------------------------------------------- #


def render_terminal(record: MapRecord, outputs: list[Path], output_dir: Path) -> str:
    line = "=" * 68
    rows = [
        ("Map name", record.map_name),
        ("Map ID", record.map_id),
        ("Votes", str(record.votes)),
        ("Map description", record.map_description),
        ("Map designer", record.map_designer),
    ]
    width = max(len(label) for label, _ in rows)
    out = [
        line,
        " Plazma Burst 2 - authenticated map scrape",
        line,
        f" Source          : {record.source_url}",
        f" Page ID         : {record.map_page_id}",
        f" Authenticated as: {record.authenticated_as} (uid {record.authenticated_uid})",
        f" Scraped at      : {record.scraped_at}",
        "-" * 68,
    ]
    for label, value in rows:
        out.append(f" {label.ljust(width)} : {value}")
    out.append("-" * 68)
    out.append(" Exports:")
    for path in outputs:
        try:
            size = path.stat().st_size
        except OSError:
            size = 0
        out.append(f"   {path.relative_to(output_dir.parent) if output_dir.parent in path.parents else path}"
                   f"  ({size} bytes)")
    out.append(line)
    return "\n".join(out)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Log in to plazmaburst2.com and scrape a custom map page.",
    )
    parser.add_argument("--map-id", help=f"numeric map page id (default {DEFAULT_MAP_ID})")
    parser.add_argument("--output-dir", help=f"export directory (default {DEFAULT_OUTPUT_DIR})")
    parser.add_argument("--base-url", help=f"site root (default {DEFAULT_BASE_URL})")
    parser.add_argument("--login", help="override PB2_LOGIN (prefer .env)")
    parser.add_argument("--password", help="override PB2_PASSWORD (prefer .env)")
    parser.add_argument("-v", "--verbose", action="store_true", help="debug logging")
    parser.add_argument("-q", "--quiet", action="store_true", help="errors only")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else (logging.ERROR if args.quiet else logging.INFO),
        format="%(levelname)s %(message)s",
        stream=sys.stderr,
    )
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    try:
        settings = load_settings(args)
    except ScrapeError as exc:
        log.error("%s", exc)
        return exc.exit_code

    client = HttpClient(settings)
    try:
        session = login(client)
        assert_authenticated(client)

        html, url = fetch_map_page(client)
        record = parse_map_page(html, url, settings.map_id, session)

        # Export only after a proven login, a fetched page and a clean parse.
        downloader = AssetDownloader(client, referer=url)
        offline_html = build_offline_html(html, url, downloader)

        stem = f"map_{settings.map_id}"
        files: dict[str, bytes] = {
            f"{stem}.json": json.dumps(
                [record.to_json()], ensure_ascii=False, indent=2
            ).encode("utf-8"),
            f"{stem}_page.html": html,  # byte-verbatim copy of the response
            f"{stem}_offline.html": offline_html.encode("utf-8"),
        }
        for name, data in downloader.assets.items():
            files[f"assets/{name}"] = data

        outputs = commit_exports(settings.output_dir, files)
    except ScrapeError as exc:
        log.error("%s", exc)
        log.error("Export aborted; existing files in %s were left untouched.",
                  settings.output_dir)
        return exc.exit_code
    except KeyboardInterrupt:
        log.error("Interrupted; existing exports left untouched.")
        return 130

    print(render_terminal(record, outputs, settings.output_dir))
    if downloader.failures:
        print(f" Note: {len(downloader.failures)} asset(s) could not be "
              f"downloaded and keep their remote URL: "
              f"{', '.join(sorted(set(downloader.failures)))}")
    print(f" {client.request_count} HTTP requests, min interval "
          f"{settings.min_interval}s, TLS verification on.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
