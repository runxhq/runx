"""Exercise the browser effect boundary without starting Brave or contacting Reddit."""
import sys
sys.dont_write_bytecode = True
import unittest
from pathlib import Path
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[2] / 'connectors/reddit/tools/reddit/brave'
sys.path.insert(0, str(TOOLS))
import publishing

BINDING = dict(account_id='example_builder', profile_ref='reddit-builder')
DATA = dict(kind='comment', account='example_builder', parameters={'profile_ref':'reddit-builder'},
            target_url='https://old.reddit.com/r/programming/comments/abc123/topic/', body='check the request id')
PERMALINK = DATA['target_url'] + 'def456/'
RECORD = dict(id='t1_def456', author='example_builder', body=DATA['body'],
              parent_id='t3_abc123', permalink=PERMALINK)

class Stop(Exception): pass

class Browser:
    BrowserStop = Stop
    EffectUnknown = Stop
    PAGE_JS = 'page'
    def __init__(self, failure=None, record=None):
        self.failure, self.record, self.clicks = failure, record or RECORD.copy(), 0
        self.reads, self.navigations = [], []
    def foreground_target(self, binding): return self.failure == 'foreground'
    def read_identity(self, binding):
        if self.failure == 'identity': raise Stop('wrong_visible_account')
    def navigate_tab(self, binding, url): self.navigations.append(url)
    def execute(self, binding, js):
        if js == 'page': return {'url': DATA['target_url']}
        data = __import__('json').loads(js.rsplit('redditForm(', 1)[1][:-2])
        command = data['command']; self.reads.append(command)
        if command == 'comments': return {'comments': [self.record] if self.clicks else []}
        if command == 'prepare':
            if self.failure == 'editor': return {'error': 'background_plain_text_editor_unavailable'}
            return {'body': DATA['body'], 'title': ''}
        if command in {'submit','vote'}:
            self.clicks += 1
            if self.failure == 'timeout': raise Stop('apple_event_timeout')
            return {'submitted': True}
        if command == 'readback': return self.record
        if command == 'vote-state':
            return {'author':'another_author', 'body':DATA['body'], 'vote_state':'up'}
        raise AssertionError(command)

class BrowserBoundary(unittest.TestCase):
    def setUp(self):
        self.sleep = patch.object(publishing.time, 'sleep'); self.sleep.start(); self.addCleanup(self.sleep.stop)
    def test_pre_effect_stops_refuse_without_clicking(self):
        for reason in ['foreground','identity','editor']:
            with self.subTest(reason=reason):
                b=Browser(reason); self.assertIn('refused', publishing.perform(b,BINDING,DATA));self.assertEqual(b.clicks,0)
    def test_acknowledgement_is_separate_from_readback(self):
        b=Browser(); ack=publishing.perform(b,BINDING,DATA)
        self.assertEqual(b.clicks,1); self.assertNotIn('readback',b.reads)
        result=publishing.readback(b,BINDING,DATA,ack)
        self.assertEqual(result['body'], DATA['body']);self.assertEqual(result['visibility'],'visible_to_account')
        self.assertEqual(b.clicks,1); self.assertEqual(b.navigations[-1],PERMALINK)
    def test_timeout_never_claims_refusal_or_repeats_submit(self):
        b=Browser('timeout')
        with self.assertRaisesRegex(Stop,'publication_outcome_unknown'): publishing.perform(b,BINDING,DATA)
        self.assertEqual(b.clicks,1)
    def test_wrong_readback_never_confirms(self):
        for field,value in [('id','t1_other'),('author','other_account'),('body','edited'),('parent_id','t1_other')]:
            with self.subTest(field=field):
                b=Browser(record={**RECORD,field:value})
                with self.assertRaises(Stop): publishing.readback(b,BINDING,DATA,{'permalink':PERMALINK})
                self.assertEqual(b.clicks,0)
    def test_acknowledgement_cannot_redirect_to_other_thread(self):
        b=Browser()
        with self.assertRaisesRegex(Stop,'publication_thread_mismatch'):
            publishing.readback(b,BINDING,DATA,{'permalink':PERMALINK.replace('abc123','another')})
        self.assertEqual(b.navigations,[])
    def test_profile_mismatch_stops_before_navigation(self):
        b=Browser();self.assertIn('refused',publishing.perform(b,BINDING,{**DATA,'parameters':{'profile_ref':'another'}}));self.assertEqual(b.navigations,[])
    def test_existing_upvote_is_not_toggled(self):
        b=Browser(); data={**DATA,'kind':'upvote','target_author':'another_author'}
        ack=publishing.perform(b,BINDING,data); result=publishing.readback(b,BINDING,data,ack)
        self.assertEqual(result['vote_state'],'up');self.assertEqual(b.clicks,0)
    def test_self_vote_is_refused(self):
        b=Browser(); data={**DATA,'kind':'upvote','target_author':BINDING['account_id']}
        self.assertIn('refused',publishing.perform(b,BINDING,data));self.assertEqual(b.clicks,0)
    def test_target_parser_preserves_exact_reply_parent(self):
        self.assertEqual(publishing.target(PERMALINK)['parent_id'],'t1_def456')
        self.assertEqual(publishing.target(DATA['target_url'])['parent_id'],'t3_abc123')
        for url in [DATA['target_url']+'?x=1',DATA['target_url'].replace('old.reddit.com','reddit.com.evil.test'),DATA['target_url'].replace('https:','http:')]:
            with self.assertRaises(ValueError): publishing.target(url)


class ProcessProtocol(unittest.TestCase):
    def run_tool(self, request, root):
        import os, subprocess, json
        command = "import sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); import brave; brave.BINDINGS_PATH=Path(sys.argv[2]); sys.argv=sys.argv[:1]; sys.exit(brave.entry())"
        return subprocess.run([sys.executable, '-B', '-c', command, str(TOOLS), str(root/'bindings.json')],
                              env={**os.environ, 'RUNX_INPUTS_JSON':json.dumps(request)}, text=True, capture_output=True)
    def test_known_pre_effect_stops_are_clean_refusals(self):
        import tempfile, fcntl, json
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            request={'operation':'comment-create','account':'example_builder','session_account':'another_account',
                     'phase':'perform','idempotency_key':'fixture-only','body':'test', 'target_url':DATA['target_url']}
            result=self.run_tool(request,root)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout),{'refused':'native_session_account_mismatch'})
            request['session_account']='example_builder'
            with (root/'bindings.lock').open('w') as lock:
                fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
                result=self.run_tool(request,root)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(json.loads(result.stdout),{'refused':'browser_operator_busy'})
            request.pop('idempotency_key')
            result=self.run_tool(request,root)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout),{'refused':'native_mutation_protocol_required'})

if __name__ == '__main__': unittest.main()
