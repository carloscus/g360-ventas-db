// Build mínimo del frontend (sin bundler): copia index.html a dist/
// Tauri empaqueta frontend/dist (ver tauri.conf.json → frontendDist).
// Uso:
//   node build.js          → sincroniza dist/ con la fuente
//   node build.js --check  → solo verifica que estén sincronizados (CI/pre-build)
const fs = require("fs");
const path = require("path");

const src = path.join(__dirname, "index.html");
const dst = path.join(__dirname, "dist", "index.html");
const check = process.argv.includes("--check");

const srcData = fs.readFileSync(src);

if (check) {
  if (!fs.existsSync(dst)) {
    console.error("FAIL: dist/index.html no existe — ejecuta: npm run build");
    process.exit(1);
  }
  const dstData = fs.readFileSync(dst);
  if (!srcData.equals(dstData)) {
    console.error(
      "FAIL: dist/index.html DESINCROMONIZADO con index.html — ejecuta: npm run build\n" +
        `  fuente: ${srcData.length} bytes\n  dist:   ${dstData.length} bytes`
    );
    process.exit(1);
  }
  console.log(`OK: dist/index.html sincronizado (${srcData.length} bytes)`);
  process.exit(0);
}

fs.mkdirSync(path.dirname(dst), { recursive: true });
fs.writeFileSync(dst, srcData);
console.log(`OK: index.html -> dist/index.html (${srcData.length} bytes)`);
