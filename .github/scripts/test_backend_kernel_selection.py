import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest


class BackendKernelSelectionTests(unittest.TestCase):
    def run_step(self, inventory_change=None, summary=None):
        workflow = (Path(__file__).parents[1] / "workflows/backend_ci.yaml").read_text()
        start = workflow.index('          kernel_inventory=')
        end = workflow.index('\n      - ', start)
        script = 'set -euo pipefail\n' + textwrap.dedent(workflow[start:end])
        cases = re.findall(r'            (service::recorded::tests::\w+)', workflow[start:end])
        self.assertEqual(len(cases), 3)
        inventory = [f'{case}: test' for case in cases]
        inventory.append('service::recorded::tests::cpu_kernel_recognizes_discontinuous_retained_speech: test')
        if inventory_change:
            inventory_change(inventory)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'loofah-cpu-stt').mkdir()
            fake = root / 'cargo'
            fake.write_text(f'#!{sys.executable}\n' + textwrap.dedent('''
                import json, os, pathlib, sys
                root = pathlib.Path(os.environ['RUNNER_TEMP'])
                args = sys.argv[1:]
                with (root / 'calls.jsonl').open('a') as calls:
                    calls.write(json.dumps(args) + '\\n')
                if '--list' in args:
                    print(os.environ['TEST_INVENTORY'])
                else:
                    assert '--exact' in args and '--ignored' in args
                    assert 'cpu_kernel_recognizes_discontinuous_retained_speech' not in ' '.join(args)
                    print(os.environ['TEST_SUMMARY'])
            '''))
            fake.chmod(0o700)
            result = subprocess.run(['bash', '-c', script], env={**os.environ,
                'PATH': f'{root}{os.pathsep}{os.environ["PATH"]}',
                'RUNNER_TEMP': str(root), 'TEST_INVENTORY': '\n'.join(inventory),
                'TEST_SUMMARY': summary or 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 80 filtered out; finished in 1s',
            }, capture_output=True, text=True)
            calls = [json.loads(line) for line in (root / 'calls.jsonl').read_text().splitlines()]
            return result, calls

    def test_each_configured_case_runs_exactly_once_without_manual_fixture(self):
        result, calls = self.run_step()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 4)
        self.assertEqual(len({call[call.index('--lib') + 1] for call in calls[1:]}), 3)

    def test_missing_or_duplicate_inventory_case_refuses_dispatch(self):
        for change in (lambda lines: lines.pop(0), lambda lines: lines.append(lines[0])):
            result, calls = self.run_step(inventory_change=change)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(len(calls), 1)

    def test_zero_multiple_ignored_or_missing_execution_never_counts_as_success(self):
        for summary in ('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured;',
                        'test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured;',
                        'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured;',
                        'test executable returned without a result'):
            result, calls = self.run_step(summary=summary)
            self.assertNotEqual(result.returncode, 0, summary)
            self.assertEqual(len(calls), 2)


if __name__ == '__main__':
    unittest.main()
