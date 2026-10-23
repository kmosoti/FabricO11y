"""DOM prerequisites only; actual AT, speech and synthesis remain separate gates."""

def assertive_filter_error(snapshot):
    expected=dict(role='alert',live='assertive',atomic='true',
        text='Filter exceeds 4096 UTF-8 bytes. Edit it before querying.')
    return isinstance(snapshot,dict) and all(snapshot.get(key)==value
        for key,value in expected.items())
