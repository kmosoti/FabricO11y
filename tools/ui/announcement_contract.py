"""Independent finite alert and target geometry joins; no native evidence producer."""
import math
from announcement_oracle import require,spoken_text


def check_alert(case,events,speech,backend):
    require(case['role']=='alert' and case['live'] in (None,'assertive') and case['focus_elsewhere'] is True,'announcement-alert-dom')
    before,after,text,pid=(case[name] for name in ('before_ns','after_ns','text','browser_pid'))
    require(backend['module']=='espeak-ng' and backend['pid']>0 and backend['libespeak_mapped'] is True,'announcement-backend')
    require(backend['pcm_after_bytes']>backend['pcm_before_bytes']>=0,'announcement-synthesis')
    require(any(row['pid']==pid and before<=row['monotonic_ns']<=after and text in row['text']
        and 'xml-roles:alert' in row.get('attributes',[]) and any(value in row.get('attributes',[]) for value in ('live:assertive','container-live:assertive'))
        and row['type'].startswith(('object:text-changed','object:property-change','object:children-changed','object:announcement')) for row in events),'announcement-alert-event')
    require(any(before<=row['before_ns']<=row['after_ns']<=after and text in spoken_text(row['text'])
        and row['module']=='espeak-ng' and row['ssip_code']==225 and row['message_id']>0 for row in speech),'announcement-utterance')


def square_fits(row):
    width,height=row['width'],row['height'];radii=row['radii']
    if width<24 or height<24:return False
    factor=min([1]+[size/total for size,total in ((width,radii[0][0]+radii[1][0]),(width,radii[2][0]+radii[3][0]),
        (height,radii[0][1]+radii[3][1]),(height,radii[1][1]+radii[2][1])) if total])
    for rx,ry in radii:
        x,y=width/2-12,height/2-12
        rx*=factor;ry*=factor
        if rx and ry and x<rx and y<ry and ((x-rx)/rx)**2+((y-ry)/ry)**2>1:return False
    return True


def grade_targets(rows):
    require(0<len(rows)<=512 and len({row['id'] for row in rows})==len(rows),'announcement-target-inventory')
    for row in rows:
        require(all(isinstance(row[key],(int,float)) and math.isfinite(row[key]) for key in ('x','y','width','height'))
            and row['width']>0 and row['height']>0 and len(row['radii'])==4
            and all(len(pair)==2 and all(math.isfinite(value) and value>=0 for value in pair) for pair in row['radii'])
            and row['transform']=='none' and row['clip']=='none','announcement-target-geometry')
    small={row['id'] for row in rows if not square_fits(row)};graded=[]
    for row in rows:
        reason='size'
        if row['id'] in small:
            inline=row.get('inline',{})
            if (row['kind']=='a' and inline.get('display')=='inline' and inline.get('ancestor') in ('p','li')
                    and inline.get('context','').strip() and inline['context'].strip()!=row['text'].strip()):
                reason='inline-text'
            else:
                cx,cy=row['x']+row['width']/2,row['y']+row['height']/2
                for other in rows:
                    if other['id']==row['id']:continue
                    dx=max(other['x']-cx,0,cx-other['x']-other['width']);dy=max(other['y']-cy,0,cy-other['y']-other['height'])
                    require(dx*dx+dy*dy>=144,'announcement-target-spacing:'+row['id'])
                    if other['id'] in small:
                        ox,oy=other['x']+other['width']/2,other['y']+other['height']/2
                        require((cx-ox)**2+(cy-oy)**2>=576,'announcement-target-spacing:'+row['id'])
                reason='spacing'
        graded.append(dict(row,qualification=reason))
    return graded
