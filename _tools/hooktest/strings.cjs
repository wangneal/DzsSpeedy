// Extract ASCII/UTF-16 strings from a PE and print matches of interest
const fs = require('fs');
const file = process.argv[2];
const buf = fs.readFileSync(file);
const out = [];
const ascii = buf.toString('latin1');
const re = /[\x20-\x7e]{6,}/g;
let m;
const interesting = /hook|Inject|LoadLibrary|CreateRemote|QueueUser|APC|MapView|Manual|Thread|NtCreate|Message|PostMessage|SendMessage|WinEvent|SetWindows|AppInit|kernel32|ntdll|user32|GetProcAddress|VirtualAlloc|WriteProcess/i;
while ((m = re.exec(ascii)) !== null) {
  const s = m[0];
  if (interesting.test(s)) out.push(s);
}
// UTF-16LE strings
const u16 = [];
for (let i = 0; i + 1 < buf.length; i += 2) {
  if (buf[i] >= 0x20 && buf[i] <= 0x7e && buf[i + 1] === 0) {
    let j = i; let s = '';
    while (j + 1 < buf.length && buf[j] >= 0x20 && buf[j] <= 0x7e && buf[j + 1] === 0) { s += String.fromCharCode(buf[j]); j += 2; }
    if (s.length >= 6 && interesting.test(s)) u16.push(s);
    i = j;
  }
}
const uniq = [...new Set([...out, ...u16])];
console.log(uniq.join('\n'));