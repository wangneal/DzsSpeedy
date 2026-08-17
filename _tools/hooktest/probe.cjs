const https = require('https');
const targets = [
  'https://github.com/ziglang/zig/releases/download/0.13.0/zig-windows-x86_64-0.13.0.zip',
  'https://objects.githubusercontent.com/',
  'https://registry.npmmirror.com/-/binary/zig/0.13.0/zig-windows-x86_64-0.13.0.zip',
  'https://mirrors.huaweicloud.com/zig/',
  'https://mirror.sjtu.edu.cn/zig/',
  'https://mirrors.cloud.tencent.com/zig/',
  'https://download.ziglang.org/0.13.0/zig-windows-x86_64-0.13.0.zip',
  'https://mirrors.ustc.edu.cn/msys2/distrib/x86_64/msys2-base-x86_64-20240507.tar.xz',
  'https://mirrors.tuna.tsinghua.edu.cn/msys2/distrib/x86_64/msys2-base-x86_64-20240507.tar.xz',
];
let i = 0;
function next() {
  if (i >= targets.length) return;
  const u = targets[i++];
  const t0 = Date.now();
  const req = https.get(u, { headers: { 'User-Agent': 'Mozilla/5.0' }, timeout: 12000 }, (res) => {
    const len = res.headers['content-length'] || '?';
    console.log(`${res.statusCode} len=${len} ${Date.now()-t0}ms ${u}`);
    res.destroy();
    next();
  });
  req.on('error', (e) => {
    console.log(`ERR ${e.code || e.message} ${Date.now()-t0}ms ${u}`);
    next();
  });
  req.on('timeout', () => { req.destroy(new Error('timeout')); });
}
next();