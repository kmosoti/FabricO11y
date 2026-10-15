"""Literal independent #45 migration observations, never a product decoder."""
CANARY = 'fabric-legacy-migration-canary-v1'
FORMAT_ERROR = 'fabric-server: access state predates the three-role format; stop the service, remove state_dir/access and re-bootstrap the owner'

def require(value, message):
    if not value: raise ValueError(message)

def grade(row):
    expected = row['legacy']['rows']
    require(len(expected) == 1 and expected[0]['body'] == CANARY
            and expected[0]['node'] == 'migration-edge', 'migration original canary')
    for name in ('legacy', 'owner', 'reader', 'cli'):
        require(row[name]['rows'] == expected and row[name]['next_page'] is None,
                'migration exact complete canary: ' + name)
    require(row['master'] == (401, 'sign in required') and row['admin_status'] == 404,
            'migration exact old-master refusal')
    require(row['format_exit'] == 1 and FORMAT_ERROR in row['format_stderr'].splitlines(),
            'migration exact format1 refusal')
    require(row['stopped_unchanged'] is True and row['prefixes_preserved'] is True
            and row['format_unchanged'] is True, 'migration original state preservation')
    return True
