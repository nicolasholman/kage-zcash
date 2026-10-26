import childProcess from 'node:child_process';
import {syncBuiltinESMExports} from 'node:module';
import {appendFileSync,mkdtempSync,openSync,closeSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';import {join} from 'node:path';
const originalSpawn=childProcess.spawn,originalExec=childProcess.execFileSync;
childProcess.spawn=function(...args){const p=originalSpawn.apply(this,args);const name=String(args[0]).split('/').at(-1);const path=new URL('./live-pids.jsonl',import.meta.url);appendFileSync(path,JSON.stringify({name,pid:p.pid,event:'start'})+'\n');p.on('exit',(code,signal)=>appendFileSync(path,JSON.stringify({name,pid:p.pid,event:'exit',code,signal})+'\n'));return p;};
// The existing live test reads keygen's output itself. Route that child stdout through a
// private /tmp file first; no private key or token is included in audit logs or tool output.
childProcess.execFileSync=function(file,args,options){if(String(file).endsWith('x402z-facilitator')&&args?.[0]==='keygen'){const dir=mkdtempSync(join(tmpdir(),'kage-exec-keygen-'));const path=join(dir,'private-output');const fd=openSync(path,'wx',0o600);try{originalExec.call(this,file,args,{...options,stdio:['ignore',fd,'pipe']});const bytes=readFileSync(path);return options?.encoding?bytes.toString(options.encoding):bytes;}finally{closeSync(fd);rmSync(dir,{recursive:true,force:true});}}return originalExec.apply(this,arguments);};
syncBuiltinESMExports();
