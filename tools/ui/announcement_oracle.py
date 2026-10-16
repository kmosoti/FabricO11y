"""Independent public-text event/speech/synthesis joins; never a DOM-only pass."""
import re
import xml.etree.ElementTree as ET


def require(value,reason):
    if not value:raise ValueError(reason)


def spoken_text(raw):
    require(isinstance(raw,str) and len(raw.encode())<=4096,'announcement-speech-format')
    if not raw.lstrip().startswith('<'):return raw
    require(raw==raw.strip() and '<!' not in raw and '<?' not in raw,'announcement-speech-format')
    try:root=ET.fromstring(raw)
    except ET.ParseError as error:raise ValueError('announcement-speech-format') from error
    require(root.tag=='speak' and not root.attrib,'announcement-speech-format')
    for node in root:
        require(node.tag=='mark' and set(node.attrib)=={'name'}
            and re.fullmatch(r'[0-9]{1,10}:[0-9]{1,10}',node.attrib['name']) is not None
            and not list(node) and not node.text,'announcement-speech-format')
    return ''.join(root.itertext())


def check_announcement(case,events,speech,backend):
    before,after=case['before_ns'],case['after_ns'];text=case['text'];pid=case['browser_pid']
    require(case['role']=='status' and case['live']=='polite' and case['focus_elsewhere'] is True,'announcement-dom')
    require(backend['module']=='espeak-ng' and backend['pid']>0 and backend['libespeak_mapped'] is True,'announcement-backend')
    require(backend['pcm_after_bytes']>backend['pcm_before_bytes']>=0,'announcement-synthesis')
    require(any(row['pid']==pid and before<=row['monotonic_ns']<=after and text in row['text']
                and row['type'].startswith(('object:text-changed','object:property-change','object:children-changed','object:announcement'))
                for row in events),'announcement-event')
    require(any(before<=row['before_ns']<=row['after_ns']<=after and text in spoken_text(row['text'])
                and row['module']=='espeak-ng' and row['ssip_code']==225 and row['message_id']>0
                for row in speech),'announcement-utterance')
