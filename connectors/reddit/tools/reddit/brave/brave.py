#!/usr/bin/env python3
"""Bound, read-only Brave adapter for the Runx Reddit skill.

This host tool never accepts caller-supplied JavaScript and never submits a
Reddit form. It reads only the selected account tab through Brave Apple Events.
"""

import argparse
import fcntl
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
ACCOUNT_RE = re.compile(r"^[A-Za-z0-9_-]{3,20}$")
ID_RE = re.compile(r"^[0-9]+$")
PROFILE_RE = re.compile(r"^[A-Za-z0-9_.:-]{1,100}$")

PAGE_JS = "JSON.stringify({url:location.href,title:document.title,ready:document.readyState,loggedIn:document.querySelector('shreddit-app')?.getAttribute('user-logged-in')==='true'})"
OPEN_MENU_JS = "(function(){const b=document.querySelector('#expand-user-drawer-button');if(!b)return JSON.stringify({error:'unsupported_identity_control'});const opened=b.getAttribute('aria-expanded')!=='true';if(opened)b.click();return JSON.stringify({openedByTool:opened})})()"
READ_IDENTITY_JS = "(function(){const b=document.querySelector('#expand-user-drawer-button');const a=Array.from(document.querySelectorAll('#user-drawer-content a[href^=\"/user/\"]')).filter(x=>x.textContent.includes('View Profile'));return JSON.stringify({open:b?.getAttribute('aria-expanded')==='true',profiles:a.map(x=>x.getAttribute('href'))})})()"
CLOSE_MENU_JS = "(function(){const b=document.querySelector('#expand-user-drawer-button');if(b?.getAttribute('aria-expanded')==='true')b.click();return JSON.stringify({closed:true})})()"
FEED_JS = "JSON.stringify({url:location.href,posts:Array.from(document.querySelectorAll('shreddit-post')).slice(0,12).map(e=>({title:e.getAttribute('post-title'),score:e.getAttribute('score'),comments:e.getAttribute('comment-count'),created:e.getAttribute('created-timestamp'),permalink:e.getAttribute('permalink'),type:e.getAttribute('post-type'),author:e.getAttribute('author')}))})"
RULES_JS = "JSON.stringify({url:location.href,rules:(function(){const headings=Array.from(document.querySelectorAll('h2')).filter(e=>/^Rule [0-9]+:/.test(e.textContent.trim()));return Array.from(document.querySelectorAll('[id^=\"rule-\"]')).slice(0,20).map((e,i)=>({title:headings[i]?.textContent.trim()||'',body:e.textContent.trim().slice(0,1600)}))})()})"
THREAD_JS = "JSON.stringify((function(){const p=document.querySelector('shreddit-post');if(!p)return {error:'post_not_rendered'};const c=Array.from(document.querySelectorAll('shreddit-comment'));const body=Array.from(p.querySelectorAll('[slot=\"text-body\"],[slot=\"content\"]')).map(e=>e.textContent.trim()).find(Boolean)||'';return {url:location.href,post:{title:p.getAttribute('post-title'),score:p.getAttribute('score'),comments:p.getAttribute('comment-count'),created:p.getAttribute('created-timestamp'),permalink:p.getAttribute('permalink'),type:p.getAttribute('post-type'),author:p.getAttribute('author'),body:body.slice(0,2400)},loadedComments:c.length,replies:c.filter(e=>e.getAttribute('depth')==='0').slice(0,30).map(e=>({author:e.getAttribute('author'),id:e.getAttribute('thingid'),score:e.getAttribute('score'),body:(e.querySelector('[slot=\"comment\"]')?.textContent||'').trim().slice(0,1500)}))}})())"
COMPOSER_JS = "JSON.stringify((function(){const p=document.querySelector('shreddit-post');const e=document.querySelector('shreddit-composer [contenteditable=\"true\"][name=\"body\"]');const b=document.querySelector('#comment-composer-submit-button');return {url:location.href,postId:p?.id||null,editorPresent:!!e,editorTextLength:e?.innerText.length||0,submitPresent:!!b,submitDisabled:b?.disabled===true,locked:!!document.querySelector('[data-post-locked=\"true\"]')}})())"


class BrowserStop(Exception):
    pass


def apple_string(value):
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('\r', '').replace('\n', '\\n') + '"'


def osa(source):
    try:
        result = subprocess.run(["osascript", "-e", source], capture_output=True, text=True, timeout=15, check=False)
    except subprocess.TimeoutExpired as error:
        raise BrowserStop("apple_event_timeout") from error
    if result.returncode:
        if "brave_not_running" in result.stderr:
            raise BrowserStop("brave_not_running")
        raise BrowserStop("apple_event_failed: " + result.stderr.strip()[:220])
    return result.stdout.strip()


def tab_source(window_id, tab_id, action):
    if not ID_RE.fullmatch(window_id) or not ID_RE.fullmatch(tab_id):
        raise BrowserStop("invalid_binding_ids")
    return ('if not (running of application id "com.brave.Browser") then error "brave_not_running"\n'
            'tell application id "com.brave.Browser"\n'
            f'  set w to first window whose id is "{window_id}"\n'
            '  if mode of w is not "normal" then error "not_normal_window"\n'
            '  set t to missing value\n'
            '  repeat with tabIndex from 1 to count of tabs of w\n'
            '    set candidate to tab tabIndex of w\n'
            f'    if id of candidate is "{tab_id}" then\n'
            '      set t to candidate\n'
            '      exit repeat\n'
            '    end if\n'
            '  end repeat\n'
            '  if t is missing value then error "missing_bound_tab"\n'
            f'  {action}\nend tell')


def execute(binding, js):
    raw = osa(tab_source(binding["window_id"], binding["tab_id"], "return execute t javascript " + apple_string(js)))
    try:
        return json.loads(raw)
    except ValueError as error:
        raise BrowserStop("invalid_browser_result") from error


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
    if osa('return running of application id "com.brave.Browser"') != "true":
        raise BrowserStop("brave_not_running")


def ensure_account(value):
    if not ACCOUNT_RE.fullmatch(value):
        raise BrowserStop("invalid_account_id")
    return value.lower()


def ensure_reddit_url(value, for_identity=False):
    parsed = urlsplit(value)
    if parsed.scheme != "https" or parsed.hostname != "www.reddit.com" or parsed.username or parsed.password or parsed.port:
        raise BrowserStop("non_reddit_url")
    if for_identity:
        return value
    if not re.fullmatch(r"/(?:r/[A-Za-z0-9_]+|user/[A-Za-z0-9_-]+)(?:/[^?#]*)?", parsed.path):
        raise BrowserStop("unsupported_reddit_url")
    return value


def read_identity(binding):
    if foreground_target(binding):
        raise BrowserStop("bound_tab_is_foreground")
    page = execute(binding, PAGE_JS)
    ensure_reddit_url(page.get("url", ""), for_identity=True)
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
    if (not isinstance(binding.get("window_id"), str) or not ID_RE.fullmatch(binding["window_id"])
            or not isinstance(binding.get("tab_id"), str) or not ID_RE.fullmatch(binding["tab_id"])
            or not isinstance(binding.get("profile_ref"), str) or not PROFILE_RE.fullmatch(binding["profile_ref"])):
        raise BrowserStop("invalid_binding_file")
    return binding


def tab_rows():
    source = '''if not (running of application id "com.brave.Browser") then error "brave_not_running"
tell application id "com.brave.Browser"
  set rows to {}
  repeat with w in windows
    if mode of w is "normal" then
      repeat with t in tabs of w
        set u to URL of t
        if u starts with "https://www.reddit.com/" then
          set end of rows to (id of w as text) & (character id 9) & (id of t as text) & (character id 9) & u
        end if
      end repeat
    end if
  end repeat
  set AppleScript's text item delimiters to (character id 10)
  return rows as text
end tell'''
    rows = []
    for line in osa(source).splitlines()[:30]:
        parts = line.split("\t", 2)
        if len(parts) == 3 and ID_RE.fullmatch(parts[0]) and ID_RE.fullmatch(parts[1]):
            rows.append({"window_id": parts[0], "tab_id": parts[1], "url": parts[2]})
    return rows


def foreground_target(binding):
    try:
        app = osa('tell application "System Events" to get name of first application process whose frontmost is true')
        if app != "Brave Browser":
            return False
        raw = osa('if not (running of application id "com.brave.Browser") then error "brave_not_running"\n'
                  'tell application id "com.brave.Browser" to return (id of front window as text) & ":" & (id of active tab of front window as text)')
        return raw == binding["window_id"] + ":" + binding["tab_id"]
    except BrowserStop:
        raise BrowserStop("foreground_state_unknown")


def navigate_tab(binding, url):
    url = ensure_reddit_url(url)
    if foreground_target(binding):
        raise BrowserStop("bound_tab_is_foreground")
    before = execute(binding, PAGE_JS)
    if before.get("url") == url and before.get("ready") == "complete":
        read_identity(binding)
        return {**before, "navigation": "already_at_url"}
    osa(tab_source(binding["window_id"], binding["tab_id"], "set URL of t to " + apple_string(url)))
    expected = urlsplit(url)
    for _ in range(10):
        time.sleep(0.7)
        page = execute(binding, PAGE_JS)
        actual = urlsplit(page.get("url", ""))
        if (page.get("ready") == "complete"
                and actual.scheme == expected.scheme
                and actual.hostname == expected.hostname
                and actual.path.rstrip("/") == expected.path.rstrip("/")
                and actual.query == expected.query):
            break
    else:
        raise BrowserStop("navigation_unverified")
    read_identity(binding)
    return {**page, "navigation": "loaded"}


def output(account_id, data):
    print(json.dumps({"account_id": account_id, "observed_at": datetime.now(timezone.utc).isoformat(), **data}, ensure_ascii=False))


def runx_argv():
    if len(sys.argv) > 1:
        return None
    raw = os.environ.get("RUNX_INPUTS_JSON")
    path = os.environ.get("RUNX_INPUTS_PATH")
    if not raw and path:
        try:
            source = Path(path)
            if source.stat().st_size > 16000:
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
        for field in ("window_id", "tab_id", "profile_ref"):
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


def main():
    parser = argparse.ArgumentParser(description="Bound read-only Brave operator for the Runx Reddit skill")
    sub = parser.add_subparsers(dest="operation", required=True)
    sub.add_parser("tabs")
    bind = sub.add_parser("bind")
    bind.add_argument("account")
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
    args = parser.parse_args(runx_argv())
    ensure_brave_running()
    if args.operation == "tabs":
        print(json.dumps({"tabs": tab_rows()}))
        return
    account_id = ensure_account(args.account)
    if args.operation == "bind":
        if not PROFILE_RE.fullmatch(args.profile_ref):
            raise BrowserStop("invalid_profile_ref")
        binding = {"account_id": account_id, "window_id": args.window_id, "tab_id": args.tab_id, "profile_ref": args.profile_ref}
        page, _ = read_identity(binding)
        data = read_bindings()
        for other, candidate in list(data.items()):
            if isinstance(candidate, dict) and other != account_id and candidate.get("window_id") == args.window_id and candidate.get("tab_id") == args.tab_id:
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
        if not result.get("rules"):
            raise BrowserStop("rules_not_rendered")
        output(account_id, {"status": "observed", **result})
    elif args.operation == "thread":
        result = execute(binding, THREAD_JS)
        if result.get("error"):
            raise BrowserStop(result["error"])
        output(account_id, {"status": "observed", **result})
    elif args.operation == "composer":
        result = execute(binding, COMPOSER_JS)
        output(account_id, {"status": "observed", **result})


if __name__ == "__main__":
    try:
        BINDINGS_PATH.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        lock_path = BINDINGS_PATH.with_suffix(".lock")
        lock_descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
        with os.fdopen(lock_descriptor, "r+"):
            try:
                fcntl.flock(lock_descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise BrowserStop("browser_operator_busy") from error
            main()
    except BrowserStop as error:
        print(json.dumps({"status": "needs_browser", "reason": str(error)}))
        sys.exit(2)
    except OSError:
        print(json.dumps({"status": "needs_browser", "reason": "binding_file_write_failed"}))
        sys.exit(2)
