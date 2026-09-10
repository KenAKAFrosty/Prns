import { router } from "expo-router";
import { act, fireEvent, renderRouter, screen } from "expo-router/testing-library";
import { Text } from "react-native";

import { NavigationLink } from "./navigation-link";

const label = "Manage a node with a long descriptive name";

function renderLink() {
  return renderRouter({
    index: () => <NavigationLink href="/target">{label}</NavigationLink>,
    target: () => <Text>Destination screen</Text>,
  });
}

describe("NavigationLink with real router composition", () => {
  it("retains row layout and allows the label to shrink without shrinking the arrow", () => {
    renderLink();

    expect(screen.getByRole("link", { name: label })).toHaveStyle({
      alignItems: "center",
      flexDirection: "row",
      minHeight: 48,
    });
    expect(screen.getByText(label)).toHaveStyle({ flex: 1, flexShrink: 1, minWidth: 0 });
    expect(screen.queryByText("→")).toBeNull();
    expect(screen.getByText("→", { includeHiddenElements: true })).toHaveStyle({
      flexShrink: 0,
    });
  });

  it("keeps press feedback and a visible focus state after asChild merging", () => {
    renderLink();

    const link = screen.getByRole("link", { name: label });
    expect(link).toHaveStyle({ backgroundColor: "#ffffff", borderColor: "#bdc9c1" });
    fireEvent(link, "pressIn");
    expect(link).toHaveStyle({ backgroundColor: "#d7eee0", flexDirection: "row" });
    fireEvent(link, "pressOut");
    expect(link).toHaveStyle({ backgroundColor: "#ffffff" });
    fireEvent(link, "focus");
    expect(link).toHaveStyle({ borderColor: "#006caa", flexDirection: "row" });
    fireEvent(link, "blur");
    expect(link).toHaveStyle({ borderColor: "#bdc9c1" });
  });

  it("still navigates through the router and preserves back navigation", () => {
    const view = renderLink();

    fireEvent.press(screen.getByRole("link", { name: label }));

    expect(view.getPathname()).toBe("/target");
    expect(screen.getByText("Destination screen")).toBeVisible();
    act(() => router.back());
    expect(view.getPathname()).toBe("/");
    expect(screen.getByRole("link", { name: label })).toBeVisible();
  });
});
