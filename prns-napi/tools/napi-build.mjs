import { spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"

const args = process.argv.slice(2)
const targetIndex = args.indexOf("--target")
const targetArgument = args.find((argument) => argument.startsWith("--target="))
const target = targetIndex === -1
  ? targetArgument?.slice("--target=".length)
  : args[targetIndex + 1]
const isGnuLinux = target
  ? target.endsWith("-unknown-linux-gnu")
  : process.platform === "linux"
const env = { ...process.env }

if (isGnuLinux) {
  const header = fileURLToPath(new URL("./glibc-compat.h", import.meta.url))
  env.CC_SHELL_ESCAPED_FLAGS = "1"
  env.CFLAGS = [
    env.CFLAGS,
    "-std=gnu99",
    `-include ${JSON.stringify(header)}`,
  ].filter(Boolean).join(" ")
}

const cli = fileURLToPath(
  new URL("../node_modules/@napi-rs/cli/dist/cli.js", import.meta.url),
)
const result = spawnSync(process.execPath, [cli, "build", ...args], {
  env,
  stdio: "inherit",
})

if (result.error) {
  throw result.error
}

process.exit(result.status ?? 1)
