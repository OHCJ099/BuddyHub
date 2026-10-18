// Runs when importing a changed encrypted login, inside the installed WorkBuddy runtime.
// Plaintext is sent through a private pipe to BuddyHub, never written to disk.
const fs = require('node:fs');
const crypto = require('node:crypto');
function decodeLogin(document, secret) {
  const key = crypto.createHash('sha256').update(secret, 'utf8').digest();
  const keyId = crypto.createHash('sha256').update(key).digest('hex').slice(0, 16);
  const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
  const lp = s => Buffer.concat([u32(Buffer.byteLength(s)), Buffer.from(s)]);
  function visit(value) {
    if (!value || typeof value !== 'object') return value;
    if (value.$wbEncrypted === 1) {
      if (value.scheme || typeof value.envelope !== 'string') throw Error('unsupported');
      const e = JSON.parse(Buffer.from(value.envelope, 'base64').toString('utf8'));
      if (e.suite !== 1 || e.keyId !== keyId) throw Error('key mismatch');
      const nonce = Buffer.from(e.nonce, 'base64'), tag = Buffer.from(e.authTag, 'base64');
      if (nonce.length !== 12 || tag.length !== 16) throw Error('invalid envelope');
      const aad = Buffer.concat([Buffer.from('WB-AAD\0'), Buffer.from([1]),
        lp('WBEV1'), lp('sym-v1'), u32(1), lp(keyId), Buffer.from([2, 0, 0])]);
      const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce);
      decipher.setAAD(aad); decipher.setAuthTag(tag);
      return Buffer.concat([decipher.update(Buffer.from(e.ciphertext, 'base64')), decipher.final()]).toString('utf8');
    }
    if (Array.isArray(value)) return value.map(visit);
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, visit(v)]));
  }
  try { return visit(document); } finally { key.fill(0); }
}
module.exports = {decodeLogin};
if (process.env.BUDDYHUB_IMPORT_LOGIN === '1') {
  try {
    const input = fs.readFileSync(0);
    if (input.length > 1024 * 1024) throw Error('too large');
    const payload = JSON.parse(process._linkedBinding('electron_browser_workbuddy_storage').loggerGet());
    if (payload.version !== 1 || typeof payload.atRestSecretKey !== 'string') throw Error('unsupported key');
    process.stdout.write(JSON.stringify(decodeLogin(JSON.parse(input), payload.atRestSecretKey)));
  } catch (_) { process.stderr.write('WorkBuddy encrypted login import unavailable'); process.exitCode = 1; }
}
