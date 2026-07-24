type LiteralTag<Name extends string> = string extends Name
  ? never
  : Name extends "UNTAGGED"
    ? never
    : Name;

export interface Tag<Name extends string, Data = undefined> {
  readonly tag: Name;
  readonly data: Data;
}

export function Tag<const Name extends string>(
  tag: LiteralTag<Name>,
): Tag<Name, undefined>;
export function Tag<const Name extends string, const Data>(
  tag: LiteralTag<Name>,
  data: Data,
): Tag<Name, Data>;
export function Tag<const Name extends string, const Data>(
  tag: LiteralTag<Name>,
  data?: Data,
): Tag<Name, Data | undefined> {
  return { tag, data };
}

export type TagFrom<Value> = [Value] extends [Tag<infer Name, unknown>]
  ? Name
  : never;

export type DataFrom<Value> = [Value] extends [Tag<string, infer Data>]
  ? Data
  : never;

type Tagged<Value> = Value extends Tag<string, unknown> ? Value : never;

type Literal<Value> = Value extends string
  ? string extends Value
    ? never
    : Value
  : never;

type Untagged<Value> = Value extends Tag<string, unknown>
  ? never
  : Value extends string
    ? string extends Value
      ? Value
      : never
    : Value;

type TagNames<Value> = Value extends Tag<infer Name, unknown> ? Name : never;

type TagData<Value, Name extends string> = Extract<
  Tagged<Value>,
  Tag<Name, unknown>
>["data"];

type HandlerNames<Value> =
  | TagNames<Value>
  | (Literal<Value> & string)
  | ([Untagged<Value>] extends [never] ? never : "UNTAGGED");

type MatchHandlers<Value> = {
  [Name in HandlerNames<Value>]: Name extends TagNames<Value>
    ? Name extends Literal<Value>
      ? (data: TagData<Value, Name> | undefined) => unknown
      : (data: TagData<Value, Name>) => unknown
    : Name extends "UNTAGGED"
      ? (value: Untagged<Value>) => unknown
      : () => unknown;
};

type MatchReturn<Handlers> = {
  [Name in keyof Handlers]: Handlers[Name] extends (
    ...args: never[]
  ) => infer Returned
    ? Returned
    : never;
}[keyof Handlers];

type RuntimeHandler = (...args: unknown[]) => unknown;

function runtimeHandler(
  handlers: object,
  name: string,
): RuntimeHandler | undefined {
  if (!Object.hasOwn(handlers, name)) {
    return undefined;
  }
  const handler = (handlers as Record<string, unknown>)[name];
  return typeof handler === "function" ? (handler as RuntimeHandler) : undefined;
}

function untaggedHandler(handlers: object): RuntimeHandler {
  const handler = runtimeHandler(handlers, "UNTAGGED");
  if (!handler) {
    throw new TypeError("casework received a value outside its declared union");
  }
  return handler;
}

export function match<const Value, Handlers extends MatchHandlers<Value>>(
  value: Value,
  handlers: Handlers,
): MatchReturn<Handlers> {
  if (typeof value === "string") {
    if (value === "UNTAGGED") {
      return untaggedHandler(handlers)(value) as MatchReturn<Handlers>;
    }
    const handler = runtimeHandler(handlers, value);
    return (
      handler ? handler() : untaggedHandler(handlers)(value)
    ) as MatchReturn<Handlers>;
  }
  if (
    typeof value === "object" &&
    value !== null &&
    "tag" in value &&
    typeof value.tag === "string"
  ) {
    const handler = runtimeHandler(handlers, value.tag);
    if (handler) {
      const data = "data" in value ? value.data : undefined;
      return handler(data) as MatchReturn<Handlers>;
    }
  }
  return untaggedHandler(handlers)(value) as MatchReturn<Handlers>;
}

type MatchIntoHandlers<Tags extends Tag<string, unknown>, Returned> = {
  [Name in TagFrom<Tags>]: (
    data: Extract<Tags, Tag<Name, unknown>>["data"],
  ) => Returned;
};

export function match_into<Returned>() {
  return {
    from<Tags extends Tag<string, unknown>>(
      tagged: Tags,
      handlers: MatchIntoHandlers<Tags, Returned>,
    ): Returned {
      const handler = runtimeHandler(handlers, tagged.tag);
      if (!handler) {
        throw new TypeError("casework received a tag outside its declared union");
      }
      return handler(tagged.data) as Returned;
    },
  };
}

export function from<Tags extends Tag<string, unknown>>() {
  function MakeTag<
    Name extends TagFrom<Tags>,
    FullTag extends Extract<Tags, Tag<Name, unknown>>,
  >(
    tag: Name,
    ...args: FullTag["data"] extends undefined ? [] : [FullTag["data"]]
  ): FullTag {
    return { tag, data: args[0] } as FullTag;
  }

  return { MakeTag };
}
