"""Independent public-text event/speech/synthesis joins; never a DOM-only pass."""


def require(value,reason):
    if not value:raise ValueError(reason)


def check_announcement(case,events,speech,backend):
    before,after=case['before_ns'],case['after_ns'];text=case['text'];pid=case['browser_pid']
    require(case['role']=='status' and case['live']=='polite' and case['focus_elsewhere'] is True,'announcement-dom')
    require(backend['module']=='espeak-ng' and backend['pid']>0 and backend['libespeak_mapped'] is True,'announcement-backend')
    require(backend['pcm_after_bytes']>backend['pcm_before_bytes']>=0,'announcement-synthesis')
    require(any(row['pid']==pid and before<=row['monotonic_ns']<=after and text in row['text']
                and row['type'].startswith(('object:text-changed','object:property-change','object:children-changed','object:announcement'))
                for row in events),'announcement-event')
    require(any(before<=row['before_ns']<=row['after_ns']<=after and text in row['text']
                and row['module']=='espeak-ng' and row['ssip_code']==225 and row['message_id']>0
                for row in speech),'announcement-utterance')
