export const contactFormDefault = { version: 1 as const, uri: "", alias: "" };
export function decodeContactForm(value: unknown): typeof contactFormDefault {
  if (typeof value !== "object" || value === null || !("version" in value) || value.version !== 1 ||
      !("uri" in value) || typeof value.uri !== "string" || value.uri.length > 4096 ||
      !("alias" in value) || typeof value.alias !== "string" || value.alias.length > 1024) {
    throw new Error("The saved contact draft cannot be read. It has been preserved.");
  }
  return { version: 1, uri: value.uri, alias: value.alias };
}

export const contactAliasDefault = { version: 1 as const, alias: undefined as string | undefined };
export function decodeContactAlias(value: unknown): typeof contactAliasDefault {
  if (typeof value !== "object" || value === null || !("version" in value) || value.version !== 1 ||
      ("alias" in value && value.alias !== undefined && (typeof value.alias !== "string" || value.alias.length > 1024))) {
    throw new Error("The saved contact-name draft cannot be read. It has been preserved.");
  }
  return { version: 1, alias: "alias" in value ? value.alias as string | undefined : undefined };
}
