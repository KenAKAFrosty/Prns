import {
  Tag as CaseworkTag,
  from,
  match,
  match_into,
} from "casework/briefcase";
import type {
  DataFrom,
  Tag as CaseworkTagged,
  TagFrom,
} from "casework/briefcase";

type LiteralTag<Name extends string> = string extends Name ? never : Name;

export type Tag<Name extends string, Data = undefined> = CaseworkTagged<Name, Data>;

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
  return data === undefined
    ? CaseworkTag(tag)
    : CaseworkTag(tag, data);
}

export { from, match, match_into };
export type { DataFrom, TagFrom };
