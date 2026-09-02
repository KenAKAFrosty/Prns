import type { ReactNode } from "react";
import { render, waitFor } from "@testing-library/react-native";

import { NodesScreen } from "@/features/nodes/nodes-screen";
import { NotYetImplementedScreen } from "@/features/placeholder-screen";
import { screenById } from "@/navigation/catalog";
import { ScaffoldStateProvider } from "@/state/scaffold-state-context";
import { developmentFingerprint } from "@/testkit/fixtures";

const mockReplace = jest.fn();

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useRouter: () => ({
    back: jest.fn(),
    push: jest.fn(),
    replace: mockReplace,
  }),
}));

describe("foundation scaffold screens", () => {
  beforeEach(() => {
    mockReplace.mockClear();
  });

  it("renders an honest placeholder without a synthetic result", () => {
    const view = render(<NotYetImplementedScreen entry={screenById("inbox.index")} />);

    expect(view.getByText("Not yet implemented")).toBeTruthy();
    expect(view.getByText("LXMF and mailbox services have not been added.")).toBeTruthy();
    expect(view.queryByText(/success/iu)).toBeNull();
    expect(view.getByRole("button", { name: "Go back" })).toBeTruthy();
  });

  it("keeps the managed fixture name and unmistakable fingerprint visible", async () => {
    const view = render(
      <ScaffoldStateProvider>
        <NodesScreen />
      </ScaffoldStateProvider>,
    );

    await waitFor(() => expect(view.getByText("E290 managed-node preview")).toBeTruthy());
    expect(view.getByText(developmentFingerprint)).toBeTruthy();
    expect(view.getByText("Fixture target")).toBeTruthy();
    expect(
      view.getByText(/cannot pair, connect, or execute RemoteControl commands/iu),
    ).toBeTruthy();
  });
});
