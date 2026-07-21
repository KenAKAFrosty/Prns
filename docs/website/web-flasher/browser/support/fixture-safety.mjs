import { lstat, readFile, readdir } from "node:fs/promises";
import path from "node:path";

const FORBIDDEN_KEY_NAMES = [
  /^id_(?:rsa|dsa|ecdsa|ed25519)$/i,
  /(?:^|[._-])(?:private|secret)(?:[._-]|$)/i,
  /(?:^|[._-])signing[._-]?key(?:[._-]|$)/i,
  /\.(?:key|pem|p12|pfx|jks|keystore)$/i,
];

const PRIVATE_KEY_MARKERS = [
  /-----BEGIN (?:OPENSSH |RSA |EC |DSA )?PRIVATE KEY-----/i,
  /(?:^|\n)untrusted comment: minisign (?:encrypted )?secret key\b/i,
  /\bAGE-SECRET-KEY-/,
];

export async function safeFixtureFiles(root) {
  const rootMetadata = await lstat(root);
  if (rootMetadata.isSymbolicLink() || !rootMetadata.isDirectory()) {
    throw new Error(`fixture root must be a real directory: ${root}`);
  }
  const output = [];
  await visit(root, root, output);
  return output;
}

async function visit(root, directory, output) {
  for (const name of (await readdir(directory)).sort()) {
    const entry = path.join(directory, name);
    const relative = path.relative(root, entry).split(path.sep).join("/");
    const metadata = await lstat(entry);
    if (metadata.isSymbolicLink()) {
      throw new Error(`fixture cannot contain a symbolic link: ${relative}`);
    }
    if (metadata.isDirectory()) {
      await visit(root, entry, output);
      continue;
    }
    if (!metadata.isFile()) {
      throw new Error(`fixture cannot contain a non-file entry: ${relative}`);
    }
    if (FORBIDDEN_KEY_NAMES.some((pattern) => pattern.test(name))) {
      throw new Error(`fixture cannot contain private-key material by name: ${relative}`);
    }
    const bytes = await readFile(entry);
    const text = bytes.toString("utf8");
    if (PRIVATE_KEY_MARKERS.some((pattern) => pattern.test(text))) {
      throw new Error(`fixture cannot contain private-key material: ${relative}`);
    }
    output.push(entry);
  }
}
