import { render, screen } from "@testing-library/react-native";
import { ContactQr } from "../src/ContactQr";

// Keep the actual QR encoder and SVG tree. Only the native drawing surface is
// provided by the React Native test renderer; the payload is never truncated.
test("the actual QR renderer accepts the maximum canonical card URI with a quiet border", async () => {
  // Canonical URI maximum: prefix16 + base64url(header84 + name128 + signature64).
  const uri = "rns-contact://v1/" + "a".repeat(368);
  const view = await render(<ContactQr uri={uri} />);
  expect(screen.getByLabelText("Your signed contact QR code")).toBeTruthy();
  const canvas = view.container.queryAll((node) => node.props.minX === -20 && node.props.minY === -20);
  expect(canvas).toHaveLength(1);
  expect(Number(canvas[0]!.props.bbWidth)).toBeGreaterThanOrEqual(140);
  const paths = () => view.container.queryAll((node) => typeof node.props.d === "string").map((node) => node.props.d);
  const original = paths();
  expect(original.length).toBeGreaterThan(0);
  expect(original.join("").length).toBeGreaterThan(100);
  await view.rerender(<ContactQr uri={uri.slice(0, -1) + "b"} />);
  expect(paths()).not.toEqual(original);
});

test("an unrenderable payload shows an accessible link fallback and a new payload can recover", async () => {
  const error = jest.spyOn(console, "error").mockImplementation(() => {});
  try {
    const view = await render(<ContactQr uri={"x".repeat(10000)} />);
    expect(screen.getByRole("alert").props.children).toContain("Share or copy the exact contact link");
    expect(screen.getByRole("alert")).toHaveStyle({ color: "#000000" });
    expect(screen.queryByLabelText("Your signed contact QR code")).toBeNull();
    await view.rerender(<ContactQr uri="rns-contact://v1/valid-sized-payload" />);
    expect(screen.getByLabelText("Your signed contact QR code")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  } finally { error.mockRestore(); }
});
