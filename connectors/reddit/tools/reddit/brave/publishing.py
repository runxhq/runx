"""Fixed browser mutations and independent readback for the process driver.

The native kernel owns approval and custody. This module owns no retry ledger,
provider credentials or arbitrary-script input.
"""

import json
import re
import time
from pathlib import Path
from urllib.parse import urlsplit


FORMS = Path(__file__).with_name("forms.js").read_text(encoding="utf-8")


def target(value):
    parsed = urlsplit(value)
    if (parsed.scheme != "https" or parsed.hostname not in {"www.reddit.com", "old.reddit.com"}
            or parsed.username or parsed.password or parsed.port or parsed.query or parsed.fragment):
        raise ValueError("invalid_publication_target")
    match = re.fullmatch(r"/r/([A-Za-z0-9_]{2,21})/(?:submit|comments/([a-z0-9]+)(?:/[^/]+)?(?:/(?:comment/)?([a-z0-9]+))?)/?", parsed.path)
    if not match:
        raise ValueError("invalid_publication_target")
    return {"subreddit": match[1], "thread_id": "t3_" + match[2] if match[2] else "",
            "parent_id": "t1_" + match[3] if match[3] else "t3_" + match[2] if match[2] else ""}


def form(browser, binding, command, data):
    if browser.foreground_target(binding):
        raise browser.BrowserStop("bound_tab_is_foreground")
    value = browser.execute(binding, FORMS + "\nJSON.stringify(redditForm(" + json.dumps({**data, "command": command}) + "))")
    if value.get("error"):
        raise browser.BrowserStop(value["error"])
    return value


def check_identity(browser, binding, data):
    browser.read_identity(binding)
    if binding["profile"] != data.get("parameters", {}).get("profile"):
        raise browser.BrowserStop("wrong_browser_profile")


def acknowledge(binding, data, permalink):
    result = {"account_id": binding["account_id"], "profile": binding["profile"],
              "parent_url": data["target_url"], "permalink": permalink,
              "title": data.get("title", ""), "body": data["body"],
              "visibility": "visible_to_account"}
    if data["kind"] == "upvote":
        result.update(target_author=data["target_author"], vote_state="up")
    return result


def readback(browser, binding, data, acknowledgement):
    check_identity(browser, binding, data)
    permalink = acknowledgement.get("permalink", "")
    identity = target(permalink)
    expected = target(data["target_url"])
    if data["kind"] == "upvote":
        browser.navigate_tab(binding, data["target_url"])
        observed = form(browser, binding, "vote-state", {**data, **expected})
        if observed.get("vote_state") != "up":
            raise browser.BrowserStop("upvote_not_observed")
        return {"account_id": binding["account_id"], "profile": binding["profile"],
                "parent_url": data["target_url"], "permalink": data["target_url"],
                "title": "", "body": observed["body"], "visibility": "visible_to_account",
                "target_author": observed["author"].lower(), "vote_state": "up"}
    if identity["subreddit"].lower() != expected["subreddit"].lower():
        raise browser.BrowserStop("publication_community_mismatch")
    if data["kind"] == "comment" and identity["thread_id"] != expected["thread_id"]:
        raise browser.BrowserStop("publication_thread_mismatch")
    browser.navigate_tab(binding, permalink)
    content_id = identity["parent_id"] if data["kind"] == "comment" else identity["thread_id"]
    observed = form(browser, binding, "readback", {**data, **identity, "content_id": content_id})
    if observed.get("id") != content_id:
        raise browser.BrowserStop("publication_identity_mismatch")
    if observed.get("author", "").lower() != binding["account_id"]:
        raise browser.BrowserStop("publication_author_mismatch")
    if observed.get("body") != data["body"]:
        raise browser.BrowserStop("publication_body_mismatch")
    if data["kind"] == "comment" and observed.get("parent_id") != expected["parent_id"]:
        raise browser.BrowserStop("publication_parent_mismatch")
    if data["kind"] == "post" and observed.get("title") != data["title"]:
        raise browser.BrowserStop("publication_title_mismatch")
    return acknowledge(binding, data, permalink)


def perform(browser, binding, data):
    """Refuse before submit, acknowledge after it, or leave native custody unknown."""
    effect_attempted = False
    try:
        if data["kind"] not in {"comment", "post", "upvote"}:
            raise browser.BrowserStop("unsupported_publication_kind")
        if not isinstance(data.get("body"), str) or not data["body"].strip():
            raise browser.BrowserStop("empty_publication_body")
        identity = target(data["target_url"])
        if (data["kind"] == "post") != (not identity["thread_id"]):
            raise browser.BrowserStop("publication_kind_target_mismatch")
        check_identity(browser, binding, data)
        browser.navigate_tab(binding, data["target_url"])
        context = {**data, **identity}
        if data["kind"] == "upvote":
            if data.get("target_author", "").lower() == binding["account_id"]:
                raise browser.BrowserStop("self_vote_refused")
            observed = form(browser, binding, "vote-state", context)
            if observed["vote_state"] != "up":
                effect_attempted = True
                form(browser, binding, "vote", context)
            return acknowledge(binding, data, data["target_url"])
        before = form(browser, binding, "comments", context)["comments"] if data["kind"] == "comment" else []
        if any(item["author"].lower() == binding["account_id"] and item["body"] == data["body"]
               and item["parent_id"] == identity["parent_id"] for item in before):
            raise browser.BrowserStop("identical_comment_already_visible")
        prepared = form(browser, binding, "prepare", context)
        if prepared["body"] != data["body"] or prepared["title"] != data.get("title", ""):
            raise browser.BrowserStop("editor_content_mismatch")
        # A deliberate review boundary before one submit, not simulated typing.
        time.sleep(1)
        check_identity(browser, binding, data)
        page = browser.execute(binding, browser.PAGE_JS)
        if page["url"].rstrip("/") != data["target_url"].rstrip("/"):
            raise browser.BrowserStop("publication_target_changed")
        effect_attempted = True
        submitted = form(browser, binding, "submit", context)
        if submitted.get("submitted") is not True:
            raise browser.BrowserStop("submission_acknowledgement_missing")
        baseline = {item["id"] for item in before}
        for _ in range(10):
            time.sleep(0.5)
            if data["kind"] == "comment":
                items = form(browser, binding, "comments", context)["comments"]
                matches = [item for item in items if item["id"] not in baseline
                           and item["author"].lower() == binding["account_id"]
                           and item["body"] == data["body"] and item["parent_id"] == identity["parent_id"]]
                if len(matches) == 1 and matches[0]["permalink"]:
                    return acknowledge(binding, data, matches[0]["permalink"])
            else:
                page = browser.execute(binding, browser.PAGE_JS)
                if "/comments/" in page["url"]:
                    return acknowledge(binding, data, page["url"])
        raise browser.BrowserStop("publication_not_observed_after_submit")
    except (browser.BrowserStop, ValueError) as error:
        if effect_attempted:
            raise browser.EffectUnknown("publication_outcome_unknown: " + str(error)) from error
        return {"refused": str(error)}
