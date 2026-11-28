import json, time, requests, re, sys
DAEMON='http://127.0.0.1:10086/command'
SESSION='pt-inbox-20261128'
UA='Mozilla/5.0'

def call(action,args=None,timeout=90):
    r=requests.post(DAEMON,json={'action':action,'args':args or {},'session':SESSION},timeout=timeout)
    r.raise_for_status()
    return r.json()

def check(site,base):
    out={'site':site,'base':base}
    try:
        call('navigate',{'url':base.rstrip('/')+'/','newTab':True,'group_title':'PT 收件箱复核'},120)
        time.sleep(2)
        call('navigate',{'url':base.rstrip('/')+'/messages.php','newTab':False},120)
        time.sleep(2)
        ev=call('evaluate',{'code':"JSON.stringify({url:location.href,title:document.title,rows:[...document.querySelectorAll('a[href*=\"viewmessage\"]')].slice(0,15).map(a=>((a.closest('tr')||a).innerText||'').replace(/\\s+/g,' ').trim())})"},60)
        out['raw']=ev
        try:
            val=json.loads(ev.get('data',{}).get('value') or ev.get('value') or '{}')
        except Exception:
            val={}
        out.update(val)
    except Exception as e:
        out['error']=str(e)[:300]
    print(json.dumps(out,ensure_ascii=False),flush=True)
    return out

sites=[
 ('tey','https://pt.tey.cc'),
 ('sbpt','https://sbpt.link'),
 ('ptfans','https://ptfans.cc'),
 ('ptzone','https://ptzone.xyz'),
 ('cspt','https://cspt.top'),
 ('hdbao','https://hdbao.cc'),
 ('luckpt','https://pt.luckpt.de'),
 ('pthome','https://www.pthome.net'),
 ('ptsbao','https://ptsbao.club'),
 ('shuioudao','https://sewerpt.com'),
 ('tangpt','https://www.tangpt.top'),
 ('audiences','https://audiences.me'),
]
res=[]
for s,b in sites:
    res.append(check(s,b))
json.dump(res,open(r'C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34\artifacts\browser-inbox-20261128.json','w',encoding='utf-8'),ensure_ascii=False,indent=1)
try: call('close_session',{},30)
except Exception: pass
