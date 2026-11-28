import json, os, re, time, concurrent.futures as cf
import requests
from bs4 import BeautifulSoup

cfg_path=os.path.expandvars(r'%LOCALAPPDATA%\BoxSend\boxsend.json')
cookie_path=os.path.expandvars(r'%LOCALAPPDATA%\BoxSend\cookies.json')
cfg=json.load(open(cfg_path,encoding='utf-8'))
cookies=json.load(open(cookie_path,encoding='utf-8'))
ids=set('audiences baozi cangbao dragonhd hdfans hdtime hdvideo kufei lajidui lemonhd longpt luckpt ptba ptcafe ptfans pthome ptlgs ptsbao ptzone qingwa sbpt shuioudao tangpt xdy city13 crabpt cspt cyanbug dubhe hdbao itzmx nanyang novahd oshen qilin railgun tey tjupt ubits xingyunge agsvpt panda iloli muxuege fenghuang gtk xingtan haoxue'.split())
sites=[s for s in cfg['sourceSites'] if s['id'] in ids]
UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36'

def get(site):
    base=site['url'].rstrip('/')+'/'
    host=re.sub(r'https?://','',base).split('/')[0]
    s=requests.Session(); s.trust_env=False
    # prefer exact host key, then parent domain keys
    keys=[k for k in cookies if host==k or host.endswith('.'+k.lstrip('.')) or k.lstrip('.')==host]
    for k in keys:
        for c in cookies[k]:
            s.cookies.set(c['name'],c['value'])
    url=base+'messages.php'
    try:
        r=s.get(url,timeout=35,headers={'User-Agent':UA,'Accept-Language':'zh-CN,zh;q=0.9'},allow_redirects=True)
        soup=BeautifulSoup(r.text,'html.parser')
        title=(soup.title.get_text(strip=True) if soup.title else '')
        links=[]
        for a in soup.select('a[href*="viewmessage"]')[:20]:
            tr=a.find_parent('tr')
            row=' '.join(tr.get_text(' ',strip=True).split()) if tr else ' '.join(a.get_text(' ',strip=True).split())
            links.append({'t':' '.join(a.get_text(' ',strip=True).split()),'h':a.get('href'),'row':row[:280]})
        return {'site':site['id'],'host':host,'status':r.status_code,'final':r.url,'len':len(r.text),'title':title,'login':('login' in r.url.lower() or '登录' in title),'links':links}
    except Exception as e:
        return {'site':site['id'],'host':host,'status':None,'error':str(e)[:200],'links':[]}

out=[]
with cf.ThreadPoolExecutor(max_workers=8) as ex:
    for r in ex.map(get,sites):
        out.append(r)
        print(r['site'],r.get('status'),len(r.get('links',[])),r.get('title','')[:40],flush=True)
p=r'C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34\artifacts\site-message-scan3-20261128.json'
json.dump(out,open(p,'w',encoding='utf-8'),ensure_ascii=False,indent=1)
print('WROTE',p)
