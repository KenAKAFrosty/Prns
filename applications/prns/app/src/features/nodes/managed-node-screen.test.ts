import * as Bindings from "@prns-internal/expo";
import type { RemoteControlDescribeOutcome } from "@prns-internal/expo";
import { describeFailureMessage } from "./managed-node-screen";
type DescribeFailureStage = Extract<
  RemoteControlDescribeOutcome,
  { readonly tag: "Failed" }
>["inner"]["stage"];
describe("managed-node failure guidance", () => {
  const stageMessages = [
    [Bindings.RemoteControlDescribeFailureStage.Input, "This paired node is no longer available."],
    [
      Bindings.RemoteControlDescribeFailureStage.Inventory,
      "This paired node is no longer available.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Route,
      "This node is not reachable yet. Keep it on and nearby, then try again.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Link,
      "The connection check could not complete. Try again.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Identification,
      "The saved pairing could not be used with this node. Check that it is still paired, then try again.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Permission,
      "This pairing does not allow the app to view node information.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Request,
      "The node could not complete the connection check. Try again.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Timeout,
      "The node did not answer before the connection check timed out. Make sure it is on and nearby, then try again.",
    ],
    [
      Bindings.RemoteControlDescribeFailureStage.Node,
      "This device went offline before the check finished.",
    ],
  ] satisfies readonly (readonly [DescribeFailureStage, string])[];
  it.each(stageMessages)("gives %s failures consumer-facing guidance", (stage, message) => {
    expect(describeFailureMessage(stage)).toBe(message);
  });
  it("keeps route, Link, identification, request, timeout, and local-node remedies distinct", () => {
    const messages = [
      describeFailureMessage(Bindings.RemoteControlDescribeFailureStage.Route),
      describeFailureMessage(Bindings.RemoteControlDescribeFailureStage.Link),
      describeFailureMessage(Bindings.RemoteControlDescribeFailureStage.Identification),
      describeFailureMessage(Bindings.RemoteControlDescribeFailureStage.Request),
      describeFailureMessage(Bindings.RemoteControlDescribeFailureStage.Timeout),
      describeFailureMessage(Bindings.RemoteControlDescribeFailureStage.Node),
    ];
    expect(new Set(messages).size).toBe(messages.length);
    expect(messages.join(" ")).not.toMatch(/RemoteControl|upstream|signed availability|E290/u);
  });
});
