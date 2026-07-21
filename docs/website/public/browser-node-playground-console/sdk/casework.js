export function Tag(tag, data) {
    return { tag, data };
}
function runtimeHandler(handlers, name) {
    if (!Object.hasOwn(handlers, name)) {
        return undefined;
    }
    const handler = handlers[name];
    return typeof handler === "function" ? handler : undefined;
}
function untaggedHandler(handlers) {
    const handler = runtimeHandler(handlers, "UNTAGGED");
    if (!handler) {
        throw new TypeError("casework received a value outside its declared union");
    }
    return handler;
}
export function match(value, handlers) {
    if (typeof value === "string") {
        if (value === "UNTAGGED") {
            return untaggedHandler(handlers)(value);
        }
        const handler = runtimeHandler(handlers, value);
        return (handler ? handler() : untaggedHandler(handlers)(value));
    }
    if (typeof value === "object" &&
        value !== null &&
        "tag" in value &&
        typeof value.tag === "string") {
        const handler = runtimeHandler(handlers, value.tag);
        if (handler) {
            const data = "data" in value ? value.data : undefined;
            return handler(data);
        }
    }
    return untaggedHandler(handlers)(value);
}
export function match_into() {
    return {
        from(tagged, handlers) {
            const handler = runtimeHandler(handlers, tagged.tag);
            if (!handler) {
                throw new TypeError("casework received a tag outside its declared union");
            }
            return handler(tagged.data);
        },
    };
}
export function from() {
    function MakeTag(tag, ...args) {
        return { tag, data: args[0] };
    }
    return { MakeTag };
}
