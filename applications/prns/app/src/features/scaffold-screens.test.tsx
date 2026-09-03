import type { ReactNode } from "react";
import { render, waitFor } from "@testing-library/react-native";

import { NodesScreen } from "@/features/nodes/nodes-screen";
import { DevelopmentRuntimeProvider } from "@/native/development-runtime-context";
import { NotYetImplementedScreen } from "@/features/placeholder-screen";
import { screenById } from "@/navigation/catalog";

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
    const view = render(<NotYetImplementedScreen entry={screenById("inbox.message")} />);

    expect(view.getByText("Coming later")).toBeTruthy();
    expect(view.getByText("View message details and delivery status.")).toBeTruthy();
    expect(view.getByText("This feature is planned for a future update.")).toBeTruthy();
    expect(view.queryByText(/success/iu)).toBeNull();
    expect(view.getByRole("button", { name: "Go back" })).toBeTruthy();
  });

  it("reports an unsupported native provider without synthetic nodes", async () => {
    const view = render(
      <DevelopmentRuntimeProvider>
        <NodesScreen />
      </DevelopmentRuntimeProvider>,
    );

    await waitFor(() => expect(view.getByText("Nodes unavailable")).toBeTruthy());
    expect(view.getByText("Node management is not available on this platform yet.")).toBeTruthy();
    expect(view.queryByText("Fixture target")).toBeNull();
  });
});
