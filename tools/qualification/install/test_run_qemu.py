"""Named rejection must survive provenance and environment checks."""
import ast
import importlib.util
import os
from pathlib import Path
import unittest
import subprocess
import tempfile
from types import SimpleNamespace
from unittest.mock import call, patch

SPEC = importlib.util.spec_from_file_location('install_qemu', Path(__file__).with_name('run-qemu.py'))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class MutationGradingTests(unittest.TestCase):
    def test_expected_rpm_mutation_cleans_only_confirmed_stopped_scratch(self):
        main = next(node for node in ast.parse(Path(MODULE.__file__).read_text()).body
                    if isinstance(node, ast.FunctionDef) and node.name == 'main')
        final = next(node.finalbody for node in main.body if isinstance(node, ast.Try))
        index = next(i for i, node in enumerate(final) if isinstance(node, ast.Assign)
                     and any(isinstance(t, ast.Name) and t.id == 'completed_mutation' for t in node.targets))
        code = compile(ast.Module(body=final[index:index + 2], type_ignores=[]), '<actual-cleanup>', 'exec')
        for output, stopped, error, expected in (
                ('ACCEPT A6 FAIL injected', True, None, True),
                ('ACCEPT A12 FAIL unrelated', True, None, False),
                ('ACCEPT A6 FAIL injected', False, None, False),
                ('ACCEPT A6 FAIL injected', True, 'helper hash mismatch', False)):
            with self.subTest(output=output, stopped=stopped, error=error), \
                    tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as root:
                namespace = dict(args=SimpleNamespace(mutate=None, expected_mutation='root-user'),
                    grading_mutation='root-user', mutation_package_archive=None,
                    mutation_rejected=MODULE.mutation_rejected, rc=1, acceptance_log=output,
                    error=error, cleanup_ok=stopped, process_cleanup_confirmed=stopped,
                    work_dir=Path(root), result_dir=Path(root), shutil=MODULE.shutil, json=MODULE.json,
                    guest_acceptance_exit=1, mutation_package_sha256=None)
                with patch.object(MODULE.shutil, 'rmtree') as remove:
                    exec(code, namespace)
                self.assertEqual(remove.call_args_list, [call(Path(root))] if expected else [])

    def test_reboot_witness_runs_after_privilege_transition(self):
        # Origin: Fedora upgrade03 really rebooted, but SSH flattened bash -c
        # argv, expanding protected paths too early and leaving a bare test.
        def fake_ssh(args, *, input_text=None, **kwargs):
            shell = '''sudo() { privileged=yes; "$@"; }
cat() {
  [ "$privileged" = yes ] || return 1
  case "$1" in
    /proc/sys/kernel/random/boot_id) echo current ;;
    /root/accept/lifecycle-boot-id) echo "$saved" ;;
    *) return 1 ;;
  esac
}
export -f cat
export privileged saved
'''
            return subprocess.run(['bash', '-c', shell + ' '.join(args[1:])],
                                  input=input_text, text=True, capture_output=True,
                                  env={'PATH': '/usr/bin:/bin', 'saved': saved})
        with patch.object(MODULE, 'run', fake_ssh):
            saved = 'before'
            self.assertEqual(MODULE.changed_boot(['fake-ssh']).returncode, 0)
            saved = 'current'
            self.assertNotEqual(MODULE.changed_boot(['fake-ssh']).returncode, 0)
            saved = 'before'
            old = fake_ssh(['fake-ssh', 'sudo', 'bash', '-c',
                           'test "$(cat /proc/sys/kernel/random/boot_id)" != "$(cat /root/accept/lifecycle-boot-id)"'])
            self.assertNotEqual(old.returncode, 0)

    def test_all_registered_named_defects_are_required(self):
        for mutation, checks in MODULE.MUTATION_EXPECTED_FAILURES.items():
            output = '\n'.join(f'ACCEPT {check} FAIL injected' for check in sorted(checks))
            self.assertTrue(MODULE.mutation_rejected(mutation, 1, output))
            for check in checks:
                with self.subTest(mutation=mutation, missing=check):
                    self.assertFalse(MODULE.mutation_rejected(
                        mutation, 1, output.replace(f'ACCEPT {check} FAIL', f'ACCEPT {check} PASS')))

    def test_unrelated_failure_is_not_detection(self):
        self.assertFalse(MODULE.mutation_rejected('root-user', 1, 'ACCEPT A12 FAIL timeout'))

    def test_bad_provenance_or_environment_cannot_pass(self):
        output = 'ACCEPT A6 FAIL injected'
        self.assertFalse(MODULE.mutation_rejected('root-user', 1, output, 'helper hash mismatch'))
        self.assertFalse(MODULE.mutation_rejected('root-user', 1, output + '\nACCEPT A8h NOT-RUN unavailable'))
        for code in (None, 0, 2, 3, -9):
            self.assertFalse(MODULE.mutation_rejected('root-user', code, output))


class InstallationShellControls(unittest.TestCase):
    """Origin: C-035 / C-aud-7; real shell decisions, isolated paths and command stubs."""
    def shell(self, program, family='fedora', *, unit='', state='inactive', avc='',
              retained=True, purge_keeps_state=False):
        with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as directory:
            root = Path(directory)
            paths = {old: root / name for old, name in (
                ('/root/accept', 'accept'), ('/var/lib/fabrico11y', 'state'),
                ('/etc/fabrico11y', 'config'), ('/usr/bin/fabric-server', 'absent-binary'))}
            work, data, config = (paths[p] for p in ('/root/accept', '/var/lib/fabrico11y', '/etc/fabrico11y'))
            work.mkdir(); config.mkdir()
            if retained: (data / 'server').mkdir(parents=True)
            account = 'fabricolly:x:995:995:Fabric O11y service::/usr/sbin/nologin\n'
            (work / 'lifecycle-account').write_text(account)
            (config / 'server.conf').write_text('synthetic configuration\n')
            for old, new in paths.items(): program = program.replace(old, str(new))
            stubs = '''
rpm() { [ "$*" = '-e fabrico11y' ]; }
dpkg() {
  if [ "$1" = -P ]; then
    if [ "$PURGE_KEEP_STATE" != yes ]; then rm -rf "$DATA"; fi
    rm -rf "$CONFIG"
  fi
}
rm() {
  [ "$1" = -rf ] || return 97; shift
  for path; do
    case "$path" in "$DATA"|"$CONFIG") command rm -rf -- "$path" ;; *) return 97 ;; esac
  done
}
package_remove() { return 0; }
getent() { printf '%s' "$ACCOUNT"; }
getenforce() { echo Enforcing; }
auditctl() { echo 'enabled 1'; }
ausearch() { printf '%s\\n' "$AVC"; }
systemctl() {
  local rc=3 value unit
  for unit; do
    case "$unit" in fabrico11y-*.service)
      value=inactive; if [ "$unit" = "$BAD_UNIT" ]; then value=$BAD_STATE; fi
      case " $* " in *' --quiet '*) ;; *) printf '%s\\n' "$value" ;; esac
      if [ "$value" = active ]; then rc=0; fi
    esac
  done
  return "$rc"
}
'''
            env = dict(os.environ, ACCOUNT=account, DATA=str(data), CONFIG=str(config),
                       BAD_UNIT=unit, BAD_STATE=state, AVC=avc,
                       PURGE_KEEP_STATE='yes' if purge_keeps_state else 'no')
            return subprocess.run(['bash', '-s', '--', 'remove', family, 'synthetic-package'],
                input=stubs + program, env=env, text=True, capture_output=True, timeout=10)

    def test_lifecycle_remove_rejects_active_services_and_fabric_avc(self):
        program = Path(__file__).with_name('lifecycle.sh').read_text()
        for unit in ('fabrico11y-server.service', 'fabrico11y-node.service'):
            for state in ('active', 'activating', 'deactivating', 'reloading'):
                with self.subTest(unit=unit, state=state):
                    result = self.shell(program, unit=unit, state=state)
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertNotIn('ACCEPT A13a PASS', result.stdout)
        for comm in ('fabric-server', 'fabric-node', 'fabricctl'):
            with self.subTest(comm=comm):
                result = self.shell(program, avc=f'type=AVC msg=audit(1.0:1): avc: denied {{ read }} comm="{comm}"')
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertNotIn('ACCEPT F4 PASS', result.stdout)
        for family in ('debian', 'fedora'):
            with self.subTest(clean=family):
                result = self.shell(program, family)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('ACCEPT A13a PASS', result.stdout)
                self.assertIn('ACCEPT A13b PASS', result.stdout)

    def test_lifecycle_remove_rejects_missing_state_and_incomplete_purge(self):
        program = Path(__file__).with_name('lifecycle.sh').read_text()
        for kwargs, forbidden in (({'retained': False}, 'ACCEPT A13a PASS'),
                                  ({'purge_keeps_state': True}, 'ACCEPT A13b PASS')):
            with self.subTest(kwargs=kwargs):
                result = self.shell(program, 'debian', **kwargs)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertNotIn(forbidden, result.stdout)

    def test_acceptance_remove_rejects_either_live_unit(self):
        source = Path(__file__).with_name('acceptance.sh').read_text()
        functions = '\n'.join(line for line in source.splitlines() if line.startswith(('pass()', 'fail()')))
        block = source[source.index('package_remove >'):source.index('package_purge >')]
        program = 'set -u\nFAILS=0\nW=/root/accept\n' + functions + '\n' + block + '\nexit "$FAILS"\n'
        for unit in ('fabrico11y-server.service', 'fabrico11y-node.service'):
            for state in ('active', 'activating', 'deactivating', 'reloading', 'inactive'):
                with self.subTest(unit=unit, state=state):
                    result = self.shell(program, unit=unit, state=state)
                    self.assertEqual(result.returncode, 0 if state == 'inactive' else 1,
                                     result.stdout + result.stderr)
                    self.assertIn('ACCEPT A13a ' + ('PASS' if state == 'inactive' else 'FAIL'), result.stdout)


if __name__ == '__main__':
    unittest.main()
