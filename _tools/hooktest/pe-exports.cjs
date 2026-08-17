// Parse PE export table. Usage: node pe-exports.cjs <file>
const fs = require('fs');
const buf = fs.readFileSync(process.argv[2]);
const u32 = (o) => buf.readUInt32LE(o);
const u16 = (o) => buf.readUInt16LE(o);
const dos = u32(0x3c);
if (buf.toString('ascii', dos, dos + 4) !== 'PE\0\0') { console.log('not PE'); process.exit(1); }
const coff = dos + 4;
const optOff = coff + 20;
const magic = u16(optOff);
const ddOff = optOff + (magic === 0x20b ? 112 : 96); // PE32+ / PE32
const expRva = u32(ddOff); // data directory[0] = export
if (!expRva) { console.log('no export directory'); process.exit(0); }
const sections = [];
const nsec = u16(coff + 2);
const secOff = optOff + (magic === 0x20b ? 240 : 224);
for (let i = 0; i < nsec; i++) {
  const o = secOff + i * 40;
  sections.push({ name: buf.toString('ascii', o, o + 8).replace(/\0/g, ''), vs: u32(o + 8), va: u32(o + 12), rs: u32(o + 16), ro: u32(o + 20) });
}
const rva2off = (rva) => {
  for (const s of sections) if (rva >= s.va && rva < s.va + s.vs) return s.ro + (rva - s.va);
  return -1;
};
const eo = rva2off(expRva);
const nNames = u32(eo + 24);
const namesRva = u32(eo + 32);
const no = rva2off(namesRva);
console.log('exports:');
for (let i = 0; i < nNames; i++) {
  const nrva = u32(no + i * 4);
  const s = rva2off(nrva);
  let end = s;
  while (buf[end] !== 0) end++;
  console.log(' ', buf.toString('ascii', s, end));
}