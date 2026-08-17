// Node download helper: follows redirects, streams to a file.
// Usage: node dl.js <url> <outfile> [maxRedirects]
const https = require('https');
const http = require('http');
const fs = require('fs');

const url = process.argv[2];
const out = process.argv[3];
let redirects = parseInt(process.argv[4] || '5', 10);

function get(u, cb) {
  const mod = u.startsWith('https:') ? https : http;
  mod.get(u, { headers: { 'User-Agent': 'Mozilla/5.0' } }, cb).on('error', (e) => {
    console.error('ERR', e.message);
    process.exit(1);
  });
}

function download(u) {
  get(u, (res) => {
    if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location && redirects > 0) {
      redirects--;
      const loc = res.headers.location;
      const next = loc.startsWith('http') ? loc : new URL(loc, u).toString();
      console.log('redirect', res.statusCode, '->', next);
      download(next);
      return;
    }
    if (res.statusCode !== 200) {
      console.error('HTTP', res.statusCode, 'for', u);
      process.exit(1);
    }
    const len = parseInt(res.headers['content-length'] || '0', 10);
    console.log('status 200, content-length =', len);
    const ws = fs.createWriteStream(out);
    let got = 0;
    res.on('data', (c) => { got += c.length; ws.write(c); });
    res.on('end', () => { ws.end(); console.log('done', got, 'bytes ->', out); });
    ws.on('error', (e) => { console.error('WRITE ERR', e.message); process.exit(1); });
  });
}

download(url);