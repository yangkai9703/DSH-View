// Minimal CDP client over raw WebSocket (no deps). Usage:
//   node cdp_probe.js <wsUrl> [expr1 expr2 ...]
// Prints console/exception events for ~1.5s first, then evaluates each expr.
const http = require('http');
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');

const shotPath = path.join(os.tmpdir(), 'cdp_shot.png');

const wsUrl = process.argv[2];
const exprs = process.argv.slice(3);
const u = new URL(wsUrl);

const key = crypto.randomBytes(16).toString('base64');
const req = http.request({
  host: u.hostname, port: u.port, path: u.pathname + u.search,
  headers: {
    Connection: 'Upgrade', Upgrade: 'websocket',
    'Sec-WebSocket-Key': key, 'Sec-WebSocket-Version': 13,
  },
});

let msgId = 0;
const pending = new Map();
let sock;

function sendFrame(data) {
  const payload = Buffer.from(data);
  const mask = crypto.randomBytes(4);
  let header;
  const len = payload.length;
  if (len < 126) header = Buffer.from([0x81, 0x80 | len]);
  else if (len < 65536) { header = Buffer.alloc(4); header[0] = 0x81; header[1] = 0x80 | 126; header.writeUInt16BE(len, 2); }
  else { header = Buffer.alloc(10); header[0] = 0x81; header[1] = 0x80 | 127; header.writeBigUInt64BE(BigInt(len), 2); }
  const masked = Buffer.from(payload);
  for (let i = 0; i < masked.length; i++) masked[i] ^= mask[i % 4];
  sock.write(Buffer.concat([header, mask, masked]));
}

function call(method, params) {
  return new Promise((resolve) => {
    const id = ++msgId;
    pending.set(id, resolve);
    sendFrame(JSON.stringify({ id, method, params }));
  });
}

let buf = Buffer.alloc(0);
function onData(chunk) {
  buf = Buffer.concat([buf, chunk]);
  while (true) {
    if (buf.length < 2) return;
    const fin = buf[0] & 0x80, op = buf[0] & 0x0f;
    let len = buf[1] & 0x7f, off = 2;
    if (len === 126) { if (buf.length < 4) return; len = buf.readUInt16BE(2); off = 4; }
    else if (len === 127) { if (buf.length < 10) return; len = Number(buf.readBigUInt64BE(2)); off = 10; }
    if (buf.length < off + len) return;
    const payload = buf.slice(off, off + len);
    buf = buf.slice(off + len);
    if (op === 1 && fin) {
      try {
        const msg = JSON.parse(payload.toString());
        if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
        else if (msg.method === 'Runtime.consoleAPICalled') {
          console.log('[console.' + msg.params.type + ']', msg.params.args.map(a => a.value ?? a.description ?? JSON.stringify(a)).join(' '));
        } else if (msg.method === 'Runtime.exceptionThrown') {
          const d = msg.params.exceptionDetails;
          console.log('[EXCEPTION]', d.text, d.exception ? (d.exception.description || d.exception.value) : '');
        }
      } catch (e) {}
    }
  }
}

req.on('upgrade', (res, socket) => {
  sock = socket;
  socket.on('data', onData);
  (async () => {
    await call('Runtime.enable');
    await new Promise(r => setTimeout(r, 1500));
    for (const expr of exprs) {
      if (expr.startsWith('CDP:')) {
        const r = await call(expr.slice(4));
        const res = r.result;
        if (res && typeof res.data === 'string' && res.data.length > 1000) {
          fs.writeFileSync(shotPath, Buffer.from(res.data, 'base64'));
          console.log('CDP:', expr.slice(4), '=> saved', shotPath, '(' + res.data.length + ' b64 chars)');
        } else {
          console.log('CDP:', expr.slice(4), '=>', JSON.stringify(res && res.result !== undefined ? res.result : res));
        }
        continue;
      }
      const r = await call('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
      const res = r.result && r.result.result;
      console.log('EVAL:', expr.slice(0, 80), '=>', res ? JSON.stringify(res.value !== undefined ? res.value : res.description) : JSON.stringify(r.result));
    }
    process.exit(0);
  })();
});
req.on('response', (res) => { console.log('upgrade failed', res.statusCode); process.exit(1); });
req.end();
setTimeout(() => { console.log('timeout'); process.exit(2); }, 20000);
