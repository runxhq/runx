#!/usr/bin/env python3
"""Fixed, account-bound Brave operations for the Reddit connector.

Native Runx owns approval and effect custody. This tool accepts data, never
caller-supplied JavaScript, and never activates the browser.
"""

import argparse
import fcntl
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit


BINDINGS_PATH = Path.home() / ".config" / "runx" / "reddit-brave-bindings.json"
BRIDGE_SOURCE = Path(__file__).with_name("bridge.swift")
BRIDGE_CACHE = Path.home() / "Library" / "Caches" / "runx" / "reddit-brave"
ACCOUNT_RE = re.compile(r"^[A-Za-z0-9_-]{3,20}$")
ID_RE = re.compile(r"^[0-9]+$")
PROFILE_RE = re.compile(r"^[A-Za-z0-9_.:-]{1,100}$")

PAGE_JS = "JSON.stringify({url:location.href,title:document.title,ready:document.readyState,classicUsername:document.querySelector('.user a[href*=\"/user/\"]')?.textContent||'',loggedIn:document.querySelector('shreddit-app')?.getAttribute('user-logged-in')==='true'})"
OPEN_MENU_JS = "(function(){const b=document.querySelector('#expand-user-drawer-button');if(!b)return JSON.stringify({error:'unsupported_identity_control'});const opened=b.getAttribute('aria-expanded')!=='true';if(opened)b.click();return JSON.stringify({openedByTool:opened})})()"
READ_IDENTITY_JS = "(function(){const b=document.querySelector('#expand-user-drawer-button');const a=Array.from(document.querySelectorAll('#user-drawer-content a[href^=\"/user/\"]')).filter(x=>x.textContent.includes('View Profile'));return JSON.stringify({open:b?.getAttribute('aria-expanded')==='true',profiles:a.map(x=>x.getAttribute('href'))})})()"
CLOSE_MENU_JS = "(function(){const b=document.querySelector('#expand-user-drawer-button');if(b?.getAttribute('aria-expanded')==='true')b.click();return JSON.stringify({closed:true})})()"
FEED_JS = "JSON.stringify({url:location.href,posts:Array.from(document.querySelectorAll('shreddit-post')).slice(0,12).map(e=>({title:e.getAttribute('post-title'),score:e.getAttribute('score'),comments:e.getAttribute('comment-count'),created:e.getAttribute('created-timestamp'),permalink:e.getAttribute('permalink'),type:e.getAttribute('post-type'),author:e.getAttribute('author')}))})"
RULES_JS = r"""JSON.stringify((function(){
  if (!/^\/(?:r\/[A-Za-z0-9_]+\/about\/rules|mod\/[A-Za-z0-9_]+\/rules)\/?$/.test(location.pathname)) return {error:'not_rules_page'};
  const headings=Array.from(document.querySelectorAll('h2')).filter(e=>/^Rule [0-9]+:/.test(e.textContent.trim()));
  const rules=Array.from(document.querySelectorAll('[id^="rule-"]')).slice(0,20).map((e,i)=>({title:headings[i]?.textContent.trim()||'',body:e.textContent.trim().slice(0,1600)}));
  return {url:location.href,rules,text:(document.querySelector('main')||document.body).innerText.slice(0,12000)};
})())"""
THREAD_JS = "JSON.stringify((function(){const p=document.querySelector('shreddit-post');if(!p)return {error:'post_not_rendered'};const c=Array.from(document.querySelectorAll('shreddit-comment'));const body=Array.from(p.querySelectorAll('[slot=\"text-body\"],[slot=\"content\"]')).map(e=>e.textContent.trim()).find(Boolean)||'';return {url:location.href,post:{title:p.getAttribute('post-title'),score:p.getAttribute('score'),comments:p.getAttribute('comment-count'),created:p.getAttribute('created-timestamp'),permalink:p.getAttribute('permalink'),type:p.getAttribute('post-type'),author:p.getAttribute('author'),body:body.slice(0,2400)},loadedComments:c.length,replies:c.filter(e=>e.getAttribute('depth')==='0').slice(0,30).map(e=>({author:e.getAttribute('author'),id:e.getAttribute('thingid'),score:e.getAttribute('score'),body:(e.querySelector('[slot=\"comment\"]')?.textContent||'').trim().slice(0,1500)}))}})())"
COMPOSER_JS = """JSON.stringify((function(){
  const p=document.querySelector('shreddit-post, .thing.link');
  const e=document.querySelector('.commentarea > form.usertext textarea[name="text"], .commentarea form#commentform textarea[name="body"], shreddit-composer [contenteditable="true"][name="body"]');
  const b=e?.closest('form')?.querySelector('button[type="submit"],input[type="submit"]')||document.querySelector('#comment-composer-submit-button');
  return {url:location.href,postId:p?.getAttribute('data-fullname')||p?.id||null,editorPresent:!!e,editorTextLength:(e?.value??e?.innerText??'').length,submitPresent:!!b,submitDisabled:b?.disabled===true,locked:!!document.querySelector('[data-post-locked="true"],.locked-infobar')};
})())"""


class BrowserStop(Exception):
    pass


class EffectUnknown(BrowserStop):
    pass


def bridge_binary():
    """The bridge, compiled once per source digest: `swift <source>` compiles on every call."""
    try:
        binary = BRIDGE_CACHE / ("bridge-" + hashlib.sha256(BRIDGE_SOURCE.read_bytes()).hexdigest())
        if binary.exists():
            if binary.is_symlink() or binary.stat().st_uid != os.getuid() or binary.stat().st_mode & 0o022:
                raise BrowserStop("browser_bridge_permissions")
            return binary
        BRIDGE_CACHE.mkdir(parents=True, exist_ok=True, mode=0o700)
        building = binary.with_name(binary.name + ".building")
        result = subprocess.run(["swiftc", "-O", str(BRIDGE_SOURCE), "-o", str(building)],
                                capture_output=True, text=True, timeout=300, check=False)
        if result.returncode:
            raise BrowserStop("browser_bridge_build_failed")
        os.chmod(building, 0o700)
        os.replace(building, binary)
        for stale in BRIDGE_CACHE.glob("bridge-*"):
            if stale != binary:
                stale.unlink(missing_ok=True)
        return binary
    except (OSError, subprocess.TimeoutExpired) as error:
        raise BrowserStop("browser_bridge_build_failed") from error


def bridge(operation, **fields):
    binary = bridge_binary()
    try:
        result = subprocess.run([str(binary), json.dumps({"operation": operation, **fields})],
                                capture_output=True, text=True, timeout=25, check=False)
    except subprocess.TimeoutExpired as error:
        raise BrowserStop("apple_event_timeout") from error
    if result.returncode:
        raise BrowserStop("browser_bridge_failed")
    try:
        value = json.loads(result.stdout)
    except ValueError as error:
        raise BrowserStop("invalid_browser_result") from error
    if isinstance(value, dict) and value.get("error"):
        raise BrowserStop(value["error"])
    return value


def execute(binding, js):
    return bridge("execute", binding=binding, javascript=js)


def read_bindings():
    if not BINDINGS_PATH.exists():
        return {}
    try:
        if BINDINGS_PATH.is_symlink() or BINDINGS_PATH.stat().st_mode & 0o077:
            raise BrowserStop("binding_file_permissions")
        data = json.loads(BINDINGS_PATH.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as error:
        raise BrowserStop("invalid_binding_file") from error
    if not isinstance(data, dict):
        raise BrowserStop("invalid_binding_file")
    return data


def write_bindings(data):
    try:
        BINDINGS_PATH.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        temporary = BINDINGS_PATH.with_suffix(".tmp")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w") as file:
            json.dump(data, file, indent=2, sort_keys=True)
            file.write("\n")
        os.replace(temporary, BINDINGS_PATH)
    except OSError as error:
        raise BrowserStop("binding_file_write_failed") from error


def norm(value):
    return re.sub(r"\r\n?", "\n", value)


def expected_file_body(value):
    body = norm(value)
    if body.endswith("\n"):
        raise BrowserStop("body_file_terminal_newline")
    return body


def ensure_brave_running():
    if not bridge("running")["running"]:
        raise BrowserStop("brave_not_running")


def ensure_account(value):
    if not ACCOUNT_RE.fullmatch(value):
        raise BrowserStop("invalid_account_id")
    return value.lower()


def ensure_reddit_url(value, for_identity=False):
    parsed = urlsplit(value)
    if parsed.scheme != "https" or parsed.hostname not in {"www.reddit.com", "old.reddit.com"} or parsed.username or parsed.password or parsed.port:
        raise BrowserStop("non_reddit_url")
    if for_identity:
        return value
    if not re.fullmatch(r"/(?:r/[A-Za-z0-9_]+|user/[A-Za-z0-9_-]+)(?:/[^?#]*)?|/mod/[A-Za-z0-9_]+/rules/?", parsed.path):
        raise BrowserStop("unsupported_reddit_url")
    return value


def read_identity(binding):
    if foreground_target(binding):
        raise BrowserStop("bound_tab_is_foreground")
    page = execute(binding, PAGE_JS)
    ensure_reddit_url(page.get("url", ""), for_identity=True)
    if page.get("classicUsername"):
        username = page["classicUsername"].lower()
        if username != binding["account_id"]:
            raise BrowserStop("wrong_visible_account")
        return page, username
    if not page.get("loggedIn"):
        raise BrowserStop("not_logged_in")
    opening = execute(binding, OPEN_MENU_JS)
    if opening.get("error"):
        raise BrowserStop(opening["error"])
    try:
        for _ in range(5):
            evidence = execute(binding, READ_IDENTITY_JS)
            if evidence.get("open") and evidence.get("profiles"):
                break
            time.sleep(0.3)
        else:
            raise BrowserStop("identity_menu_unreadable")
        profiles = evidence["profiles"]
        if len(profiles) != 1:
            raise BrowserStop("ambiguous_visible_identity")
        match = re.fullmatch(r"/user/([A-Za-z0-9_-]+)/?", profiles[0])
        if not match:
            raise BrowserStop("invalid_visible_identity")
        username = match.group(1).lower()
        if username != binding["account_id"]:
            raise BrowserStop("wrong_visible_account")
        return page, username
    finally:
        if opening.get("openedByTool"):
            original_error = sys.exc_info()[1]
            try:
                execute(binding, CLOSE_MENU_JS)
            except BrowserStop:
                if original_error is None:
                    raise


def bound_account(account):
    account_id = ensure_account(account)
    data = read_bindings()
    binding = data.get(account_id)
    if not isinstance(binding, dict):
        raise BrowserStop("account_not_bound")
    if binding.get("account_id") != account_id:
        raise BrowserStop("binding_account_mismatch")
    if (not isinstance(binding.get("browser_pid"), str) or not ID_RE.fullmatch(binding["browser_pid"])
            or not isinstance(binding.get("window_id"), str) or not ID_RE.fullmatch(binding["window_id"])
            or not isinstance(binding.get("tab_id"), str) or not ID_RE.fullmatch(binding["tab_id"])
            or not isinstance(binding.get("profile_ref"), str) or not PROFILE_RE.fullmatch(binding["profile_ref"])):
        raise BrowserStop("invalid_binding_file")
    return binding


def tab_rows():
    return bridge("tabs")["tabs"]


def foreground_target(binding):
    return bridge("foreground", binding=binding)["foreground"]


def navigate_tab(binding, url):
    url = ensure_reddit_url(url)
    if foreground_target(binding):
        raise BrowserStop("bound_tab_is_foreground")
    before = execute(binding, PAGE_JS)
    if before.get("url") == url and before.get("ready") == "complete":
        read_identity(binding)
        return {**before, "navigation": "already_at_url"}
    bridge("navigate", binding=binding, url=url)
    expected = urlsplit(url)
    rules = re.fullmatch(r"/r/([A-Za-z0-9_]+)/about/rules/?", expected.path)
    for _ in range(10):
        time.sleep(0.7)
        page = execute(binding, PAGE_JS)
        actual = urlsplit(page.get("url", ""))
        if (page.get("ready") == "complete"
                and actual.scheme == expected.scheme
                and actual.hostname == expected.hostname
                and (actual.path.rstrip("/") == expected.path.rstrip("/")
                     or (rules and actual.path.rstrip("/") == "/mod/" + rules[1] + "/rules"))
                and actual.query == expected.query):
            break
    else:
        raise BrowserStop("navigation_unverified")
    read_identity(binding)
    return {**page, "navigation": "loaded"}


def output(account_id, data):
    print(json.dumps({"account_id": account_id, "observed_at": datetime.now(timezone.utc).isoformat(), **data}, ensure_ascii=False))


def runx_inputs():
    if len(sys.argv) > 1:
        return None
    raw = os.environ.get("RUNX_INPUTS_JSON")
    path = os.environ.get("RUNX_INPUTS_PATH")
    if not raw and path:
        try:
            source = Path(path)
            if source.stat().st_size > 128000:
                raise BrowserStop("invalid_tool_inputs")
            raw = source.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise BrowserStop("invalid_tool_inputs") from error
    if not raw:
        return None
    try:
        data = json.loads(raw)
    except ValueError as error:
        raise BrowserStop("invalid_tool_inputs") from error
    if not isinstance(data, dict) or not isinstance(data.get("operation"), str):
        raise BrowserStop("invalid_tool_inputs")
    return data


def runx_argv(data):
    if data is None:
        return None
    operation = data["operation"]
    if operation == "tabs":
        return [operation]
    if operation not in {"bind", "inspect", "navigate", "feed", "rules", "thread", "composer", "comment-readback"}:
        raise BrowserStop("invalid_tool_inputs")
    account = data.get("account")
    if not isinstance(account, str):
        raise BrowserStop("invalid_tool_inputs")
    argv = [operation, account]
    if operation == "bind":
        for field in ("browser_pid", "window_id", "tab_id", "profile_ref"):
            if not isinstance(data.get(field), str):
                raise BrowserStop("invalid_tool_inputs")
            argv.extend(["--" + field.replace("_", "-"), data[field]])
    elif operation in {"navigate", "comment-readback"}:
        if not isinstance(data.get("url"), str):
            raise BrowserStop("invalid_tool_inputs")
        argv.append(data["url"])
        if operation == "comment-readback":
            if not isinstance(data.get("body_file"), str):
                raise BrowserStop("invalid_tool_inputs")
            argv.extend(["--body-file", data["body_file"]])
    return argv


def main(data):
    if data and data["operation"] == "session":
        parameters = data.get("parameters", {})
        binding = bound_account(parameters.get("account", ""))
        if binding["profile_ref"] != parameters.get("profile_ref"):
            raise BrowserStop("wrong_browser_profile")
        _, username = read_identity(binding)
        print(json.dumps({"id": username, "login": username}))
        return
    if data and data.get("account") and data.get("session_account") != data["account"]:
        raise BrowserStop("native_session_account_mismatch")
    if data and data["operation"] in {"comment-create", "post-create", "upvote"}:
        import publishing
        data = {**data, "kind": {"comment-create": "comment", "post-create": "post", "upvote": "upvote"}[data["operation"]]}
        if data.get("phase") not in {"perform", "readback"} or not data.get("idempotency_key"):
            raise BrowserStop("native_mutation_protocol_required")
        try:
            ensure_brave_running()
            binding = bound_account(data["account"])
        except (BrowserStop, KeyError) as error:
            if data["phase"] == "perform":
                print(json.dumps({"refused": str(error)}))
                return
            raise
        if data["phase"] == "perform":
            try:
                result = publishing.perform(sys.modules[__name__], binding, data)
                print(json.dumps(result, ensure_ascii=False))
            except EffectUnknown:
                raise
            except Exception as error:
                # Once dispatched, unexpected failures are never safe refusals.
                raise EffectUnknown("browser_mutation_failed_after_dispatch") from error
        else:
            result = publishing.readback(sys.modules[__name__], binding, data, data["acknowledgement"])
            print(json.dumps(result, ensure_ascii=False))
        return
    parser = argparse.ArgumentParser(description="Bound read-only Brave operator for the Runx Reddit skill")
    sub = parser.add_subparsers(dest="operation", required=True)
    sub.add_parser("tabs")
    bind = sub.add_parser("bind")
    bind.add_argument("account")
    bind.add_argument("--browser-pid", required=True)
    bind.add_argument("--window-id", required=True)
    bind.add_argument("--tab-id", required=True)
    bind.add_argument("--profile-ref", required=True)
    for name in ("inspect", "feed", "rules", "thread", "composer"):
        sub.add_parser(name).add_argument("account")
    nav = sub.add_parser("navigate")
    nav.add_argument("account")
    nav.add_argument("url")
    comment = sub.add_parser("comment-readback")
    comment.add_argument("account")
    comment.add_argument("url")
    comment.add_argument("--body-file", required=True)
    args = parser.parse_args(runx_argv(data))
    ensure_brave_running()
    if args.operation == "tabs":
        print(json.dumps({"tabs": tab_rows()}))
        return
    account_id = ensure_account(args.account)
    if args.operation == "bind":
        if not PROFILE_RE.fullmatch(args.profile_ref):
            raise BrowserStop("invalid_profile_ref")
        binding = {"account_id": account_id, "browser_pid": args.browser_pid, "window_id": args.window_id, "tab_id": args.tab_id, "profile_ref": args.profile_ref}
        page, _ = read_identity(binding)
        data = read_bindings()
        for other, candidate in list(data.items()):
            if isinstance(candidate, dict) and other != account_id and candidate.get("browser_pid") == args.browser_pid and candidate.get("window_id") == args.window_id and candidate.get("tab_id") == args.tab_id:
                del data[other]
        data[account_id] = binding
        write_bindings(data)
        output(account_id, {"status": "bound", "url": page["url"], "profile_ref": args.profile_ref})
        return
    binding = bound_account(account_id)
    page, _ = read_identity(binding)
    if args.operation == "inspect":
        output(account_id, {"status": "observed", "page": page, "profile_ref": binding["profile_ref"]})
    elif args.operation == "navigate":
        page = navigate_tab(binding, args.url)
        output(account_id, {"status": "already_at_url" if page["navigation"] == "already_at_url" else "navigated", "page": page})
    elif args.operation == "comment-readback":
        parts = urlsplit(ensure_reddit_url(args.url)).path.strip("/").split("/")
        if len(parts) == 6:
            comment_id = parts[5]
        elif len(parts) == 7 and parts[5] == "comment":
            comment_id = parts[6]
        else:
            raise BrowserStop("expected_comment_permalink")
        if parts[0] != "r" or parts[2] != "comments" or not re.fullmatch(r"[a-z0-9]+", comment_id):
            raise BrowserStop("expected_comment_permalink")
        body_path = Path(args.body_file)
        try:
            if body_path.is_symlink() or not body_path.is_file() or body_path.stat().st_size > 10000:
                raise BrowserStop("invalid_body_file")
            expected_body = expected_file_body(body_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError) as error:
            raise BrowserStop("invalid_body_file") from error
        if not expected_body.strip():
            raise BrowserStop("empty_expected_body")
        navigate_tab(binding, args.url)
        js = "JSON.stringify((function(){const e=Array.from(document.querySelectorAll('shreddit-comment')).find(e=>e.getAttribute('thingid')==='t1_" + comment_id + "');return e?{author:e.getAttribute('author')||'',body:e.querySelector('[slot=comment]')?.innerText||''}:null})())"
        for _ in range(5):
            observed = execute(binding, js)
            if observed:
                break
            time.sleep(0.7)
        else:
            raise BrowserStop("comment_readback_missing")
        if not isinstance(observed.get("author"), str) or observed["author"].lower() != account_id:
            raise BrowserStop("comment_readback_wrong_author")
        if not isinstance(observed.get("body"), str) or norm(observed["body"]) != expected_body:
            raise BrowserStop("comment_readback_body_mismatch")
        output(account_id, {"status": "comment_visible_to_account", "url": args.url, "author": observed["author"], "body": observed["body"]})
    elif args.operation == "feed":
        result = execute(binding, FEED_JS)
        if not result.get("posts"):
            raise BrowserStop("feed_not_rendered")
        output(account_id, {"status": "observed", **result})
    elif args.operation == "rules":
        result = execute(binding, RULES_JS)
        if result.get("error") or not (result.get("rules") or result.get("text")):
            raise BrowserStop(result.get("error", "rules_not_rendered"))
        output(account_id, {"status": "observed", **result})
    elif args.operation == "thread":
        result = execute(binding, THREAD_JS)
        if result.get("error"):
            raise BrowserStop(result["error"])
        output(account_id, {"status": "observed", **result})
    elif args.operation == "composer":
        result = execute(binding, COMPOSER_JS)
        output(account_id, {"status": "observed", **result})


def entry():
    data = None
    try:
        data = runx_inputs()
        BINDINGS_PATH.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        lock_path = BINDINGS_PATH.with_suffix(".lock")
        lock_descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
        with os.fdopen(lock_descriptor, "r+"):
            try:
                fcntl.flock(lock_descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise BrowserStop("browser_operator_busy") from error
            main(data)
        return 0
    except EffectUnknown as error:
        print(json.dumps({"status": "unknown", "reason": str(error)}))
        return 2
    except (BrowserStop, OSError, ValueError, TypeError) as error:
        reason = str(error) if isinstance(error, BrowserStop) else "browser_startup_failed"
        if data and data.get("phase") == "perform" and data.get("operation") in {"comment-create", "post-create", "upvote"}:
            print(json.dumps({"refused": reason}))
            return 0
        print(json.dumps({"status": "needs_browser", "reason": reason}))
        return 2


if __name__ == "__main__":
    sys.exit(entry())
