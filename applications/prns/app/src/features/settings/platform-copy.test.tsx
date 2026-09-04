import { render } from "@testing-library/react-native";

import { AboutScreen } from "./about-screen";
import { HelpScreen } from "./help-screen";

describe("unsupported-platform capability copy", () => {
  it("identifies the web preview without showing a native identifier or native capabilities", () => {
    const view = render(<AboutScreen />);

    expect(view.getByText("Web preview")).toBeTruthy();
    expect(view.queryByText("Native identifier")).toBeNull();
    expect(
      view.getByText(/This web preview lets you explore the app's layout, navigation, settings/iu),
    ).toBeTruthy();
    expect(view.getByText(/not available on web yet/iu)).toBeTruthy();
    expect(view.queryByText(/pair and check nearby nodes/iu)).toBeNull();
  });

  it("limits web help to the interactions that web actually supports", () => {
    const view = render(<HelpScreen />);

    expect(
      view.getByText(/Browse the responsive app shell, navigate planned pages/iu),
    ).toBeTruthy();
    expect(view.getByText(/Identity setup, node pairing and status/iu)).toBeTruthy();
    expect(view.getByText(/cannot create, import, or reset app data on web/iu)).toBeTruthy();
    expect(view.queryByText(/Create or import an identity, pair and check nodes/iu)).toBeNull();
    expect(view.queryByText(/Preview data can be reset/iu)).toBeNull();
  });
});
