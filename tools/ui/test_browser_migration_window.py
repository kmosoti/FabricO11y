"""Origin: exact-package row67 HTTP400 with a legacy all-u64 console window."""
import ast
import inspect
import unittest
import browser_migration
from migration_contract import CANARY

ACTUAL=dict(node='migration-edge',body=CANARY,observed_ns=1792789624048681978)

def query(row):
    if hasattr(browser_migration,'migration_query'):return browser_migration.migration_query(row)
    tree=ast.parse(inspect.getsource(browser_migration.prepare))
    assignments=[node for node in ast.walk(tree) if isinstance(node,ast.Assign)
        and any(isinstance(target,ast.Name) and target.id=='query' for target in node.targets)]
    assert len(assignments)==1
    return eval(compile(ast.Expression(assignments[0].value),'<old migration query>','eval'),
        {'__builtins__':{},'dict':dict,'CANARY':CANARY})

class MigrationWindow(unittest.TestCase):
    def test_observed_canary_has_a_console_valid_window(self):
        for timestamp in (0,ACTUAL['observed_ns'],2**64-2):
            with self.subTest(timestamp=timestamp):
                actual=query(dict(ACTUAL,observed_ns=timestamp))
                self.assertEqual(set(actual),{'kind','node','from_ns','to_ns','contains','limit'})
                self.assertEqual((actual['kind'],actual['node'],actual['contains'],actual['limit']),
                    ('logs','migration-edge',CANARY,2))
                self.assertLessEqual(0,actual['from_ns'])
                self.assertLessEqual(actual['from_ns'],timestamp)
                self.assertLess(timestamp,actual['to_ns'])
                self.assertLessEqual(actual['to_ns'],2**64-1)
                self.assertLessEqual(actual['to_ns']-actual['from_ns'],86_400_000_000_000)

    def test_invalid_original_observation_is_not_replaced_with_a_new_canary(self):
        for key,value in [('node','other'),('body','other'),('observed_ns',True),
                ('observed_ns',-1),('observed_ns',2**64-1),('observed_ns',1.5),('observed_ns',None)]:
            with self.subTest(key=key,value=value):
                with self.assertRaises(ValueError):query(dict(ACTUAL,**{key:value}))
        bad=dict(ACTUAL);del bad['observed_ns']
        with self.assertRaises(ValueError):query(bad)

if __name__=='__main__':unittest.main()
