"""Origin: row69 private daemon idle exit and speech-idle-lifecycle-01."""
import ast
import inspect
from pathlib import Path
import unittest
import announcement_probe

def configuration():
    logs=Path('/owned/speech-logs')
    if hasattr(announcement_probe,'speech_configuration'):
        return announcement_probe.speech_configuration(logs)
    tree=ast.parse(inspect.getsource(announcement_probe.inside))
    candidates=[node.args[0] for node in ast.walk(tree)
        if isinstance(node,ast.Call) and isinstance(node.func,ast.Attribute)
        and node.func.attr=='write_text' and node.args and isinstance(node.args[0],ast.JoinedStr)
        and any(isinstance(part,ast.Constant) and isinstance(part.value,str)
            and 'AudioOutputMethod' in part.value for part in node.args[0].values)]
    assert len(candidates)==1
    return eval(compile(ast.Expression(candidates[0]),'<old speech configuration>','eval'),
        {'__builtins__':{},'speech_logs':logs})

class SpeechLifetime(unittest.TestCase):
    def test_private_daemon_is_owned_until_controller_stop(self):
        lines=configuration().splitlines()
        self.assertEqual([line for line in lines if line.startswith('Timeout ')],['Timeout 0'])

    def test_real_backend_capture_and_log_configuration_are_preserved(self):
        lines=configuration().splitlines()
        for line in ('LogLevel 2','LogDir "/owned/speech-logs"',
                'AddModule "espeak-ng" "sd_espeak-ng" "espeak-ng.conf"',
                'DefaultModule espeak-ng','AudioOutputMethod "alsa"',
                'AudioALSADevice "fabric_capture"'):
            self.assertEqual(lines.count(line),1)

if __name__=='__main__':unittest.main()
