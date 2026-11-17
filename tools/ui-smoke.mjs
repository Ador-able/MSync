import { spawn, execFileSync } from 'node:child_process';
import { mkdir, writeFile, readFile, open, utimes, readdir, stat, rename } from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
import { createServer } from 'node:net';
const root=path.resolve(import.meta.dirname,'..');
const data=path.join(root,'target','ui-smoke',String(Date.now()));
await mkdir(data,{recursive:true});
const defaultIgnore=String.raw`^~\$`;
const simulated=process.argv.includes('--simulated');
const logRetention=process.argv.includes('--log-retention');
const portable=process.argv.slice(2).find(a=>!a.startsWith('--'));
const appData=portable?path.join(data,'MSync'):data;
const logDir=path.join(appData,'logs');
if(logRetention){
  await mkdir(logDir,{recursive:true});
  const seed=async(name,size,age)=>{const file=path.join(logDir,name);const handle=await open(file,'w');await handle.truncate(size);await handle.close();const time=new Date(Date.now()-age);await utimes(file,time,time);};
  await seed('expired.log',1,8*86400000);
  await seed('launcher.log',10000001,1000);
  for(let n=0;n<10;n++)await seed(`old-${n}.log`,10000000,(20-n)*1000);
  await writeFile(path.join(logDir,'保留.txt'),'非日志文件');
  await writeFile(path.join(appData,'保留.txt'),'同步资料不清理');
}
const local=path.join(data,'电脑');const remote=path.join(data,'平板');const mode=path.join(data,'设备状态.txt');
const fault=path.join(data,'故障.json');
if(simulated){await mkdir(local,{recursive:true});await mkdir(path.join(remote,'工作文件'),{recursive:true});await writeFile(path.join(local,'测试.md'),'ADB 界面流程验证');await writeFile(path.join(local,'~$本地.doc'),'本地临时文件');await writeFile(path.join(remote,'工作文件','~$平板.doc'),'平板临时文件');await writeFile(path.join(remote,'工作文件','平板正常.md'),'下载普通文件');await writeFile(mode,'');await writeFile(fault,'{}');}
const remoteShell='/'+remote[0].toLowerCase()+remote.slice(2).replaceAll('\\','/');
const listener=createServer();await new Promise(r=>listener.listen(0,'127.0.0.1',r));const port=listener.address().port;await new Promise(r=>listener.close(r));
const launch=()=>spawn(portable ?? path.join(root,'target/debug/msync-ui.exe'),portable?[]:['--data-dir',data],{windowsHide:true,env:{...process.env,...(portable?{LOCALAPPDATA:data}:{}),...(simulated?{MSYNC_ADB_PATH:path.join(root,'target/debug/examples/adb_sync_test.exe'),MSYNC_TEST_MODE_FILE:mode,MSYNC_TEST_SHARED_ROOT:remoteShell,MSYNC_TEST_FAULT_FILE:fault}:{}),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`,WEBVIEW2_USER_DATA_FOLDER:path.join(data,'webview')}});
let child=launch();
let socket;
let openedCopies=[];
try {
  let pages;
  for(let i=0;i<120;i++){try{pages=await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();if(pages.some(p=>p.type==='page'))break;}catch{}await new Promise(r=>setTimeout(r,500));}
  const page=pages?.find(p=>p.type==='page');assert(page,'桌面 WebView 没有启动');
  socket=new WebSocket(page.webSocketDebuggerUrl);await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject;});
  let next=0;const waiting=new Map();socket.onmessage=({data})=>{const response=JSON.parse(data);if(response.id){waiting.get(response.id)?.(response);waiting.delete(response.id);}};
  socket.onclose=()=>{for(const resolve of waiting.values())resolve({error:new Error('桌面 WebView 已关闭')});waiting.clear();};
  const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++next;const timer=setTimeout(()=>{waiting.delete(id);reject(new Error('WebView 调用超时：'+method));},15000);waiting.set(id,r=>{clearTimeout(timer);r.error?reject(r.error):resolve(r.result);});socket.send(JSON.stringify({id,method,params}));});
  const evaluate=async expression=>{const r=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value;};
  const until=async (expression,checks=180)=>{for(let i=0;i<checks;i++){if(await evaluate(expression))return;await new Promise(r=>setTimeout(r,300));}const state=await evaluate('({phase:document.querySelector("#phase")?.textContent,notice:document.querySelector("#notice")?.textContent,ignoreError:document.querySelector("#ignore-error")?.textContent})');throw Error('界面流程超时：'+expression+'；状态：'+JSON.stringify(state));};
  for(let i=0;i<30;i++){if(await evaluate('!!document.querySelector("#sync")'))break;await new Promise(r=>setTimeout(r,200));}
  const click=async id=>{await until(`(()=>{const e=document.getElementById(${JSON.stringify(id)});return e && !e.disabled && e.getClientRects().length>0;})()`);await evaluate(`document.getElementById(${JSON.stringify(id)}).click()`);};
  const screenshot=async (name,selector)=>{const clip=selector?await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return {x:r.x+scrollX,y:r.y+scrollY,width:r.width,height:r.height,scale:2};})()`):undefined;const shot=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:true,...(clip?{clip}:{})});await writeFile(path.join(data,name+'.png'),Buffer.from(shot.data,'base64'));};
  const fits=async()=>assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'),true,'界面横向溢出');
  const modalFits=async id=>{
    assert.equal(await evaluate(`(()=>{const d=document.getElementById('${id}'),r=d.getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight && d.scrollHeight<=d.clientHeight+1 && getComputedStyle(document.querySelector('main')).overflowY==='hidden';})()`),true,'弹窗或背景出现了额外滚动');
  };
  assert.equal(await evaluate('document.querySelector(".brand, .mark")'),null);
  assert.equal(await evaluate('document.querySelector("#sync").disabled'),true);
  assert.equal(await evaluate('document.querySelector("#detail").hidden'),true);
  await fits();await screenshot('首页-未设置');
  await evaluate('document.querySelector("#more").click()');
  await until('document.querySelector("#more").getAttribute("aria-expanded")==="true"');
  await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
  assert.equal(await evaluate('document.querySelector("#settings").hidden'),true);
  await evaluate('document.querySelector("#logs-toggle").click()');await new Promise(r=>setTimeout(r,1200));
  assert.match(await evaluate('document.querySelector("#log-output").textContent'),/MSync.*启动/);
  await evaluate('document.querySelector("#copy-log").click()');await new Promise(r=>setTimeout(r,300));
  assert.equal(await evaluate('document.querySelector("#copy-log").textContent'),'已复制');
  assert.match(execFileSync('powershell.exe',['-NoProfile','-Command','Get-Clipboard -Raw'],{encoding:'utf8',windowsHide:true}),/MSync/);
  await modalFits('logs-panel');await screenshot('日志面板');
  await evaluate('document.querySelector("#logs-close").click()');
  await until('!document.querySelector("#logs-panel").open');
  await evaluate('document.querySelector("#usb-pick").click()');await new Promise(r=>setTimeout(r,1500));
  assert.equal(await evaluate('document.querySelector("#device-dialog").open'),true);
  const devices=await evaluate('window.__TAURI__.core.invoke("list_devices")');
  const listing=await evaluate('document.querySelector("#device-folders").textContent+[...document.querySelectorAll("#device-folders button")].map(b=>b.getAttribute("aria-label")).join(" ")');
  if(devices.length)for(const device of devices)assert(listing.includes(device.serial));
  else assert.match(listing,/未发现 ADB 设备/);
  assert.equal(await evaluate('document.querySelector("#device-back").disabled'),true);
  await modalFits('device-dialog');
  await screenshot('选择平板');
  if(simulated){
    await evaluate('document.querySelector("#device-folders button").click()');
    await until('document.querySelector("#device-folders").textContent.includes("工作文件")');
    await evaluate('[...document.querySelectorAll("#device-folders button")].find(b=>b.textContent.includes("工作文件")).click()');
    await until('!document.querySelector("#device-select").disabled');
    await evaluate('document.querySelector("#device-select").click()');
    await evaluate(`(()=>{const original=window.fetch.bind(window);window.fetch=(url,options)=>String(url).endsWith('/choose_folder')?Promise.resolve(new Response(JSON.stringify(${JSON.stringify(local)}),{headers:{'content-type':'application/json','Tauri-Response':'ok'}})):original(url,options);document.querySelector('#local-pick').click();})()`);
    await until('!document.querySelector("#sync").disabled');
    await click('sync');
    await until('!document.querySelector("#stop").hidden');
    assert.equal(await evaluate('document.querySelector("#sync").hidden'),true);
    assert.equal(await evaluate('!document.querySelector("#sync-progress").hidden && !document.querySelector("#progress-bar").hasAttribute("value")'),true,'扫描阶段应显示不定进度');
    await screenshot('同步中');
    await until('document.querySelector("#phase").textContent==="已完成"');
    // 设置验证独立于定时调度，避免首次自动检查抢占配置写入。
    await evaluate('window.__TAURI__.core.invoke("set_automatic",{enabled:false})');
    const progressStatus=await evaluate('window.__TAURI__.core.invoke("get_status")');
    assert.equal(progressStatus.progress.completed,progressStatus.progress.total);
    assert(progressStatus.elapsed_ms>0,'同步耗时未记录');
    await new Promise(r=>setTimeout(r,1100));
    assert.equal((await evaluate('window.__TAURI__.core.invoke("get_status")')).elapsed_ms,progressStatus.elapsed_ms,'完成后计时未停止');
    assert.match(await evaluate('document.querySelector("#progress-count").textContent'),/100%/);
    assert.equal(await readFile(path.join(local,'平板正常.md'),'utf8'),'下载普通文件');
    assert.equal((await readdir(local)).includes('~$平板.doc'),false);
    assert.equal((await readdir(path.join(remote,'工作文件'))).includes('~$本地.doc'),false);
    assert.equal(await evaluate('document.querySelector("#confirmation").open'),false);
    const firstShot=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:true});await writeFile(path.join(data,'首次同步.png'),Buffer.from(firstShot.data,'base64'));
    assert.equal(await evaluate('document.querySelector("#detail").hidden && document.querySelector("#local-pick").hidden && document.querySelector("#usb-pick").hidden'),true);
    const boundPaths=await evaluate('["local-path","usb-path"].map(id=>document.getElementById(id).textContent)');
    await evaluate('document.querySelector("#more").click()');
    await until('!document.querySelector("#settings").hidden');await screenshot('设置');await screenshot('说明-更多设置','#settings');
    await click('ignore-settings');await modalFits('ignore-dialog');
    assert.equal(await evaluate('document.querySelector("#ignore-patterns").value'),defaultIgnore);
    await screenshot('忽略文件');await screenshot('说明-忽略文件','#ignore-dialog');
    await evaluate('document.querySelector("#ignore-patterns").value="["');await click('ignore-save');
    await until('document.querySelector("#ignore-error").textContent.includes("第 1 行正则表达式无效")');
    assert.deepEqual((await evaluate('window.__TAURI__.core.invoke("get_status")')).pair.ignore_patterns,[defaultIgnore]);
    await evaluate(`document.querySelector("#ignore-patterns").value=${JSON.stringify(defaultIgnore+'\n'+String.raw`(?i)\.cache$`+'\n'+String.raw`(?i)\.cache$`)}`);await click('ignore-save');
    await until('!document.querySelector("#ignore-dialog").open');
    assert.deepEqual((await evaluate('window.__TAURI__.core.invoke("get_status")')).pair.ignore_patterns,[defaultIgnore,String.raw`(?i)\.cache$`]);
    await writeFile(path.join(local,'验证.Local.CACHE'),'不传输');await writeFile(path.join(remote,'工作文件','验证.Remote.cache'),'不下载');
    await writeFile(path.join(local,'正常新增.md'),'继续同步');await click('sync');
    await until('!document.querySelector("#stop").hidden');await until('document.querySelector("#phase").textContent==="已完成"');
    assert.equal(await readFile(path.join(remote,'工作文件','正常新增.md'),'utf8'),'继续同步');
    assert.equal((await readdir(local)).includes('验证.Remote.cache'),false);
    assert.equal((await readdir(path.join(remote,'工作文件'))).includes('验证.Local.CACHE'),false);
    await evaluate(`window.__TAURI__.core.invoke('set_ignore_patterns',{patterns:[${JSON.stringify(defaultIgnore)}]})`);
    await click('more');
    await evaluate('document.querySelector("#change").click()');
    await until('!document.querySelector("#editing-bar").hidden');
    assert.equal(await evaluate('document.querySelector("#more").getAttribute("aria-expanded")'),'false');
    assert.equal(await evaluate('document.querySelector("#sync").disabled'),true);
    await evaluate('document.querySelector("#cancel-change").click()');
    await until('document.querySelector("#editing-bar").hidden');
    assert.deepEqual(await evaluate('["local-path","usb-path"].map(id=>document.getElementById(id).textContent)'),boundPaths);
    assert.equal(await evaluate('document.querySelector("#sync").disabled'),false);
    await call('Emulation.setDeviceMetricsOverride',{width:540,height:400,deviceScaleFactor:1,mobile:false});
    await fits();await screenshot('最小窗口');
    assert.equal(await evaluate('document.querySelector("footer").getBoundingClientRect().bottom <= innerHeight'),true,'最小窗口首屏操作不完整');
    await call('Emulation.clearDeviceMetricsOverride');
    assert.equal(await readFile(path.join(remote,'工作文件','测试.md'),'utf8'),'ADB 界面流程验证');
    const successful=await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.last_success)');
    for(const [state,phase] of [['storage-locked','存储暂不可访问'],['unauthorized','等待授权'],['disconnected','等待设备']]){
      await writeFile(mode,state);await click('sync');
      await until(`document.querySelector('#phase').textContent===${JSON.stringify(phase)}`);
      assert.equal(await evaluate('document.querySelector("#detail").hidden'),false);
      assert.equal(await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.last_success)'),successful);
      assert.equal(await readFile(path.join(local,'测试.md'),'utf8'),'ADB 界面流程验证');
    }
    await writeFile(mode,'');
    await writeFile(path.join(local,'a-失败.md'),'等待后台补同步');
    await writeFile(path.join(local,'z-继续.md'),'其他文件继续');
    await writeFile(fault,JSON.stringify({operation:'replace',path:'/a-失败.md',remaining:3,when:'before'}));
    await click('sync');
    await until('document.querySelector("#phase").textContent==="部分文件未完成"');
    const partialProgress=await evaluate('window.__TAURI__.core.invoke("get_status")');
    assert.equal(partialProgress.progress.completed,partialProgress.progress.total,'失败文件未计入已处理进度');
    assert.equal(await readFile(path.join(remote,'工作文件','z-继续.md'),'utf8'),'其他文件继续');
    assert.match(await evaluate('document.querySelector("#failures-list").textContent'),/a-失败.md.*3 次/);
    assert.equal(await evaluate('document.querySelector("#confirmation").hidden'),true);
    await evaluate('document.querySelector("#failures-panel").open=true');
    const failureShot=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:true});await writeFile(path.join(data,'部分失败.png'),Buffer.from(failureShot.data,'base64'));
    // 不点同步按钮，验证真实 60 秒调度会自动重试，且无需恢复确认。
    await evaluate('window.__TAURI__.core.invoke("set_automatic",{enabled:true})');
    await until('document.querySelector("#phase").textContent==="已完成" && document.querySelector("#failures-panel").hidden',420);
    assert.equal(await readFile(path.join(remote,'工作文件','a-失败.md'),'utf8'),'等待后台补同步');
    assert.equal(await evaluate('document.querySelector("#confirmation").hidden'),true);
    const tablet=path.join(remote,'工作文件');
    const marker=path.join(local,'.usbsync-meta','endpoint.json');
    const originalMarker=await readFile(marker,'utf8');
    await writeFile(path.join(local,'电脑独有.md'),'电脑独有内容');
    await writeFile(path.join(tablet,'平板独有.md'),'平板独有内容');
    await writeFile(path.join(local,'测试.md'),'选择保留的电脑版');
    await writeFile(path.join(tablet,'测试.md'),'时间较新的平板版');
    await utimes(path.join(local,'测试.md'),1700000000,1700000000);
    await utimes(path.join(tablet,'测试.md'),1700000600,1700000600);
    await writeFile(marker,'损坏的 JSON');
    await writeFile(path.join(tablet,'.usbsync-meta','endpoint.json'),JSON.stringify({pair:'其他绑定',endpoint:'other',role:'平板'}));
    await click('sync');
    await until('document.querySelector("#confirmation").open');
    assert.equal(await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.preview.merge_recovery)'),true);
    assert.equal(await evaluate('document.querySelector("#conflicts fieldset").dataset.path'),'测试.md');
    assert.equal(await evaluate('document.querySelectorAll("#conflicts .version").length'),2);
    assert.equal(await evaluate('document.querySelectorAll("#conflicts select").length'),0);
    assert.equal(await evaluate('document.querySelectorAll("#conflicts input:checked").length'),0);
    const columns=await evaluate('[...document.querySelectorAll("#conflicts .version")].map(e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,right:r.right};})');
    assert.equal(columns[0].y,columns[1].y);assert(columns[0].right<=columns[1].x+0.1,'两版没有并排显示');
    await modalFits('confirmation');
    assert.equal(await evaluate('document.querySelector("#conflict-body").scrollHeight <= document.querySelector("#conflict-body").clientHeight+1'),true,'单个冲突不应需要滚动');
    assert.equal(await evaluate('document.activeElement.closest("#confirmation")!==null'),true);
    await click('confirm');
    await until('document.querySelector("#confirm-error").textContent.includes("请选择") || document.querySelector("#confirm-error").textContent.includes("选择保留")');
    assert.equal(await readFile(path.join(tablet,'测试.md'),'utf8'),'时间较新的平板版');
    assert.equal(await readFile(path.join(tablet,'电脑独有.md'),'utf8'),'电脑独有内容');
    assert.equal(await readFile(path.join(local,'平板独有.md'),'utf8'),'平板独有内容');
    // 真实点击两侧打开按钮，从磁盘独立读取副本，同时确认没有替用户选版。
    for(let side=0;side<2;side++){
      await evaluate(`document.querySelectorAll('#conflicts .open-version')[${side}].click()`);
      await until(`!document.querySelectorAll('#conflicts .open-version')[${side}].disabled`);
      assert.equal(await evaluate('document.querySelector("#confirm-error").textContent'),'');
    }
    const opened=await evaluate('window.__TAURI__.core.invoke("get_logs").then(rows=>rows.filter(r=>r.message.startsWith("打开版本副本：")).map(r=>r.message.slice("打开版本副本：".length)))');
    openedCopies=opened;
    assert.equal(opened.length,2);
    assert.equal(await readFile(opened[0],'utf8'),'选择保留的电脑版');
    assert.equal(await readFile(opened[1],'utf8'),'时间较新的平板版');
    assert.match(path.basename(opened[0]),/（电脑）\.md$/);assert.match(path.basename(opened[1]),/（平板）\.md$/);
    assert.equal(await evaluate('document.querySelectorAll("#conflicts input:checked").length'),0);
    await call('Page.bringToFront');
    await writeFile(path.join(data,'打开版本验证.json'),JSON.stringify({copies:opened,local:await readFile(path.join(local,'测试.md'),'utf8'),remote:await readFile(path.join(tablet,'测试.md'),'utf8')},null,2));
    await screenshot('左右选版');
    const beforeConflicts=await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.preview.conflicts)');
    await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await until('!document.querySelector("#confirmation").open');
    assert.equal(await readFile(marker,'utf8'),'损坏的 JSON');
    const waitingToken=await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.preview.token)');
    await writeFile(path.join(local,'待选版时新增.md'),'后台继续处理其他文件');
    await until(`window.__TAURI__.core.invoke('get_status').then(s=>!s.busy && s.preview && s.preview.token!==${JSON.stringify(waitingToken)})`,420);
    assert.equal(await readFile(path.join(tablet,'待选版时新增.md'),'utf8'),'后台继续处理其他文件');
    assert.equal(await evaluate('document.querySelector("#confirmation").open'),false,JSON.stringify({before:beforeConflicts,after:await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.preview.conflicts)')}));
    await until('!document.querySelector("#sync").disabled && !document.querySelector("#sync").hidden');
    await click('sync');
    await until('document.querySelector("#confirmation").open');
    const mergeShot=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:true});await writeFile(path.join(data,'合并选版.png'),Buffer.from(mergeShot.data,'base64'));
    await evaluate(`document.querySelector('#conflicts input[value="0"]').click()`);await click('confirm');
    await until('document.querySelector("#phase").textContent==="已完成"');
    for(const folder of [local,tablet]){
      assert.equal(await readFile(path.join(folder,'测试.md'),'utf8'),'选择保留的电脑版');
      assert.equal(await readFile(path.join(folder,'电脑独有.md'),'utf8'),'电脑独有内容');
      assert.equal(await readFile(path.join(folder,'平板独有.md'),'utf8'),'平板独有内容');
    }
    assert.deepEqual(JSON.parse(await readFile(marker,'utf8')),JSON.parse(originalMarker));
    const mergedSuccess=await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.last_success)');
    await writeFile(path.join(tablet,'测试.md'),'合并后的普通修改');
    await click('sync');
    await until(`window.__TAURI__.core.invoke('get_status').then(s=>!s.busy && s.last_success!==${JSON.stringify(mergedSuccess)} && s.phase==='已完成')`);
    assert.equal(await readFile(path.join(local,'测试.md'),'utf8'),'合并后的普通修改');
    assert.equal(await evaluate('document.querySelector("#confirmation").open'),false);
    const beforeMove=await evaluate('window.__TAURI__.core.invoke("get_status").then(s=>s.last_success)');
    const moved=path.join(data,'电脑-移走后保留');
    assert.equal(path.dirname(local),data);assert.equal(path.dirname(moved),data);
    await rename(local,moved);
    await click('sync');
    await until(`window.__TAURI__.core.invoke('get_status').then(s=>!s.busy && s.last_success!==${JSON.stringify(beforeMove)} && s.phase==='已完成')`);
    assert.equal(await readFile(path.join(local,'测试.md'),'utf8'),'合并后的普通修改');
    assert.equal(await readFile(path.join(moved,'测试.md'),'utf8'),'合并后的普通修改');
    assert.equal(await evaluate('document.querySelector("#confirmation").open'),false);
  }else await evaluate('document.querySelector("#device-close").click()');
  const shot=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:true});await writeFile(path.join(root,'target/ui-smoke.png'),Buffer.from(shot.data,'base64'));
  await writeFile(path.join(data,'界面.png'),Buffer.from(shot.data,'base64'));
  const logs=await evaluate('window.__TAURI__.core.invoke("get_logs")');assert(logs.some(l=>l.message.includes('启动')));
  if(!simulated)assert(logs.some(l=>l.message.includes('Android Debug Bridge version')),'随包 ADB 未能启动');
  if(logRetention){
    const names=await readdir(logDir);
    for(const name of ['expired.log','launcher.log','old-0.log'])assert(!names.includes(name),`旧日志未清理：${name}`);
    assert(names.includes('old-1.log'),'误删仍在限额内的日志');
    if(portable)assert(names.some(name=>name.startsWith('launcher-')),'启动器未使用新日志');
    assert(names.some(name=>name.startsWith('msync-')),'主程序未使用新日志');
    let total=0;
    for(const name of names.filter(name=>name.endsWith('.log'))){const size=(await stat(path.join(logDir,name))).size;assert(size<=10000000,`日志超过 10 MB：${name}`);total+=size;}
    assert(total<=100000000,'日志总量超过 100 MB');
    assert.equal(await readFile(path.join(logDir,'保留.txt'),'utf8'),'非日志文件');
    assert.equal(await readFile(path.join(appData,'保留.txt'),'utf8'),'同步资料不清理');
    await writeFile(path.join(data,'日志限额验证.json'),JSON.stringify({total,files:names},null,2));
  }
  if(simulated)await evaluate('window.__TAURI__.core.invoke("set_automatic",{enabled:false})');
  const closed=new Promise(resolve=>socket.addEventListener('close',resolve,{once:true}));
  await evaluate('window.__TAURI__.core.invoke("quit")').catch(()=>{});
  await closed;
  for(const file of openedCopies){
    let exists=true;
    for(let i=0;i<50;i++){try{await stat(file);}catch(e){if(e.code!=='ENOENT')throw e;exists=false;break;}await new Promise(r=>setTimeout(r,100));}
    assert.equal(exists,false,'退出后预览副本没有清理');
  }
  if(simulated){
    // 沿用刚才的绑定和进度，关闭自动同步后重启，独立验证启动清理入口。
    const markers=[];const retained=[];
    for(const folder of [local,path.join(remote,'工作文件')]){
      await mkdir(path.join(folder,'_同步备份'),{recursive:true});
      for(const name of ['.usbsync-access-old',".usbsync-access-中文 ' $",'_同步备份/.msync-owner']){
        const file=path.join(folder,name);await writeFile(file,'旧版标记');markers.push(file);
      }
      await writeFile(path.join(folder,'_同步备份','历史备份.md'),'保留的备份');
      for(const name of ['.usbsync-meta/endpoint.json','测试.md','_同步备份/历史备份.md']){
        const file=path.join(folder,name);retained.push([file,await readFile(file)]);
      }
    }
    child=launch();
    for(const file of markers){
      let exists=true;
      for(let i=0;i<120;i++){try{await stat(file);}catch(e){if(e.code!=='ENOENT')throw e;exists=false;break;}await new Promise(r=>setTimeout(r,250));}
      assert.equal(exists,false,'启动后旧版标记没有删除：'+file);
    }
    for(const [file,content] of retained)assert.deepEqual(await readFile(file),content,'启动清理改动了资料：'+file);
    let restarted;
    for(let i=0;i<120;i++){try{restarted=(await(await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(p=>p.type==='page');if(restarted)break;}catch{}await new Promise(r=>setTimeout(r,250));}
    assert(restarted,'重启后桌面 WebView 没有启动');
    socket=new WebSocket(restarted.webSocketDebuggerUrl);await new Promise(r=>socket.onopen=r);
    socket.onmessage=({data})=>{const response=JSON.parse(data);if(response.id){waiting.get(response.id)?.(response);waiting.delete(response.id);}};
    socket.onclose=()=>{for(const resolve of waiting.values())resolve({error:new Error('桌面 WebView 已关闭')});waiting.clear();};
    await until('!!window.__TAURI__?.core && !!document.querySelector("#sync")');
    const status=await evaluate('window.__TAURI__.core.invoke("get_status")');
    assert.equal(status.pair.automatic,false);assert.equal(status.phase,'已就绪');
    assert.deepEqual(status.pair.ignore_patterns,[defaultIgnore],'重启后默认规则丢失');
    const stopped=new Promise(resolve=>socket.addEventListener('close',resolve,{once:true}));
    await evaluate('window.__TAURI__.core.invoke("quit")').catch(()=>{});await stopped;
    await writeFile(path.join(data,'启动清理验证.json'),JSON.stringify({removed:markers,preserved:retained.map(([file])=>file),automatic:status.pair.automatic},null,2));
  }
  const result={result:'通过',simulated,checks:['真实 Tauri 页面','未选目录不能开始','读取 Rust 日志','复制日志反馈','ADB 设备枚举及空列表提示','日志与文件选择仅单层滚动',...(logRetention?['启动器与主程序日志统一限额','旧日志按时间/大小/总量清理','非日志资料保留']:[]),...(simulated?['绑定后直接同步','外部读取传输结果','最小窗口首屏完整','更换目录可取消','左右选版及打开两侧副本','退出后临时副本清理','关闭自动同步后重启仍清理双端旧标记','清理保留工作文件和备份及身份标记','存储不可访问/未授权/断线状态保留成功记录','单文件失败原因及其余文件继续','定时补同步','损坏及不匹配标记自动合并','未选版保留冲突并完成其他文件','Escape 关闭弹窗后后台继续且不重复弹窗','合并后修复标记并恢复增量同步','配置目录被移走后自动重建并补回文件']:[])],data,screenshot:'target/ui-smoke.png'};
  await writeFile(path.join(data,'验证结果.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
}finally{socket?.close();if(child.exitCode===null)child.kill();}
