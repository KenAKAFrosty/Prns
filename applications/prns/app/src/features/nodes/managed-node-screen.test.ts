import type { RemoteControlDescribeOutcome } from "@prns-internal/expo";

import { describeFailureMessage } from "./managed-node-screen";

type DescribeFailureStage = Extract<
  RemoteControlDescribeOutcome,
  { readonly type: "failed" }
>["stage"];

describe("managed-node failure guidance", () => {
  const stageMessages = {
    input: "This paired node is no longer available.",
    inventory: "This paired node is no longer available.",
    route: "This node is not reachable yet. Keep it on and nearby, then try again.",
    link: "The connection check could not complete. Try again.",
    identification:
      "The saved pairing could not be used with this node. Check that it is still paired, then try again.",
    permission: "This pairing does not allow the app to view node information.",
    request: "The node could not complete the connection check. Try again.",
    timeout:
      "The node did not answer before the connection check timed out. Make sure it is on and nearby, then try again.",
    node: "This device went offline before the check finished.",
  } satisfies Record<DescribeFailureStage, string>;

  it.each(Object.entries(stageMessages) as [DescribeFailureStage, string][])(
    "gives %s failures consumer-facing guidance",
    (stage, message) => {
      expect(describeFailureMessage(stage)).toBe(message);
    },
  );

  it("keeps route, Link, identification, request, timeout, and local-node remedies distinct", () => {
    const messages = [
      describeFailureMessage("route"),
      describeFailureMessage("link"),
      describeFailureMessage("identification"),
      describeFailureMessage("request"),
      describeFailureMessage("timeout"),
      describeFailureMessage("node"),
    ];

    expect(new Set(messages).size).toBe(messages.length);
    expect(messages.join(" ")).not.toMatch(/RemoteControl|upstream|signed availability|E290/u);
  });
});
