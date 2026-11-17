import {spawn, execFileSync} from 'node:child_process';
import {mkdir, writeFile, readFile} from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
import {parseArgs} from 'node:util';
const {values:args}=parseArgs({options:{serial:{type:'string'},'folder-guide':{type:'boolean',default:false}}});
assert(args.serial,'请通过 --serial 指定已授权的测试设备序列号');
const serial=args.serial;
const root=path.resolve(import.meta.dirname,'..');
const base=path.join(root,'target/manual');
const foldersOnly=args['folder-guide'];
const assets=path.join(base,'assets');
const local=path.join(base,'演示','电脑工作文件');
const remote='/storage/emulated/0/MSync说明演示-'+Date.now();
const adb=path.join(root,'runtime/adb-download/platform-tools/adb.exe');
await mkdir(local,{recursive:true});await mkdir(assets,{recursive:true});
await writeFile(path.join(local,'使用说明示例.txt'),'这是一份同步演示文件。\n');
if(!foldersOnly)execFileSync(adb,['-s',serial,'shell','-T',`mkdir -p '${remote}/工作文件'`],{windowsHide:true});
const child=spawn(path.join(root,'target/release/msync-ui.exe'),['--data-dir',path.join(base,'会话-'+Date.now())],{windowsHide:true,env:{...process.env,MSYNC_ADB_PATH:adb,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=9338',WEBVIEW2_USER_DATA_FOLDER:path.join(base,'webview')}});
let socket;
try {
 let page;
 for(let i=0;i<90;i++){try{page=(await(await fetch('http://127.0.0.1:9338/json/list')).json()).find(p=>p.type==='page');if(page)break;}catch{}await new Promise(r=>setTimeout(r,500));}
 assert(page);socket=new WebSocket(page.webSocketDebuggerUrl);await new Promise((ok,no)=>{socket.onopen=ok;socket.onerror=no;});
 let seq=0;const wait=new Map();socket.onmessage=({data})=>{const r=JSON.parse(data);if(r.id){wait.get(r.id)?.(r);wait.delete(r.id);}};
 const call=(method,params={})=>new Promise((ok,no)=>{const id=++seq;const t=setTimeout(()=>no(Error(method)),30000);wait.set(id,r=>{clearTimeout(t);r.error?no(r.error):ok(r.result);});socket.send(JSON.stringify({id,method,params}));});
 const evaluate=async expression=>{const r=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;};
 const until=async expression=>{for(let i=0;i<300;i++){if(await evaluate(expression))return;await new Promise(r=>setTimeout(r,300));}throw Error(expression);};
 const click=async id=>evaluate(`document.querySelector(${JSON.stringify(id)}).click()`);
 const shot=async(name,selector)=>{await new Promise(r=>setTimeout(r,250));const clip=selector?await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return {x:r.x+scrollX,y:r.y+scrollY,width:r.width,height:r.height,scale:2};})()`):undefined;const r=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:true,...(clip?{clip}:{})});await writeFile(path.join(assets,name+'.png'),Buffer.from(r.data,'base64'));};
 await until('!!document.querySelector("#sync")');
 await shot('01-首页');
 await click('#usb-pick');await until(`[...document.querySelectorAll("#device-folders button")].some(b=>b.title===${JSON.stringify(serial)})`);await shot('02-选择平板','#device-dialog');
 const selectDevice=()=>evaluate(`[...document.querySelectorAll("#device-folders button")].find(b=>b.title===${JSON.stringify(serial)}).click()`);
 if(foldersOnly){
   await selectDevice();await until('[...document.querySelectorAll("#device-folders button")].some(b=>b.textContent.includes("工作文件"))');
   await evaluate('[...document.querySelectorAll("#device-folders button")].find(b=>b.textContent.trim()==="工作文件").click()');
   await until('document.querySelector("#device-location").textContent.endsWith(" / 工作文件") && !document.querySelector("#device-select").disabled');
   await shot('04-确认文件夹','#device-dialog');await click('#device-select');await shot('11-约定文件夹','.folders');
   console.log('已只读确认教程目录：工作文件');await evaluate('window.__TAURI__.core.invoke("quit")').catch(()=>{});
 }else{
 await selectDevice();await until(`document.querySelector('#device-folders').textContent.includes(${JSON.stringify(remote.split('/').at(-1))})`);
 await evaluate(`[...document.querySelectorAll('#device-folders button')].find(b=>b.textContent.includes(${JSON.stringify(remote.split('/').at(-1))})).click()`);
 await until('document.querySelector("#device-folders").textContent.includes("工作文件")');await shot('03-选择文件夹','#device-dialog');
 await click('#device-folders button');await until('document.querySelector("#device-location").textContent.includes("工作文件") && !document.querySelector("#device-select").disabled');await shot('04-确认文件夹','#device-dialog');
 await click('#device-select');
 // 仅让系统选目录返回演示目录；绑定、同步和设备访问均走真实实现。
 await evaluate(`(()=>{const f=window.fetch.bind(window);window.fetch=(url,opt)=>String(url).endsWith('/choose_folder')?Promise.resolve(new Response(JSON.stringify(${JSON.stringify(local)}),{headers:{'content-type':'application/json','Tauri-Response':'ok'}})):f(url,opt);document.querySelector('#local-pick').click();})()`);
 await until('!document.querySelector("#sync").disabled');await click('#sync');await until('document.querySelector("#phase").textContent==="已完成"');
 await shot('05-首次同步');
 const pull=path.join(base,'回读.txt');execFileSync(adb,['-s',serial,'pull',remote+'/工作文件/使用说明示例.txt',pull],{windowsHide:true});assert.equal(await readFile(pull,'utf8'),'这是一份同步演示文件。\n');
 await evaluate('scrollTo(0,0)');await shot('06-同步完成');
 await click('#more');await shot('07-更多设置','#settings');await click('#more');
 await click('#logs-toggle');await until('document.querySelector("#log-output").textContent.includes("INFO")');await shot('08-复制日志','.log-toolbar');await click('#copy-log');await until('document.querySelector("#copy-log").textContent.includes("已复制")');
 console.log(JSON.stringify({result:'真实桌面与平板同步通过',remote,assets}));
 await evaluate('window.__TAURI__.core.invoke("quit")').catch(()=>{});
 }
}finally{socket?.close();if(child.exitCode===null)child.kill();}
