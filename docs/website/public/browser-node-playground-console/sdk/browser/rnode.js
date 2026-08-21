import { Tag } from "../casework.js";
export class RNodeInterface {
    name = "rnode";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect() {
        const ready = this.#host.runtimeReadiness();
        if (ready.tag !== "Ready") {
            return ready;
        }
        return Tag("UnsupportedInterface", {
            interface: "rnode",
            host: "Browser",
        });
    }
}
