import { fireEvent, render, waitFor } from "@testing-library/react-native";

import { OnboardingScreen } from "./onboarding-screen";

const mockBack = jest.fn();
const mockReplace = jest.fn();
const mockCreateGeneratedIdentity = jest.fn();
const mockCreateImportedIdentity = jest.fn();
const mockPreviewIdentityImport = jest.fn();
const mockGetDocumentAsync = jest.fn();
const mockReadBytes = jest.fn();
const mockDelete = jest.fn();

jest.mock("expo-router", () => ({
  useRouter: () => ({
    back: mockBack,
    replace: mockReplace,
  }),
}));

jest.mock("expo-document-picker", () => ({
  getDocumentAsync: (...input: readonly unknown[]) => mockGetDocumentAsync(...input),
}));

jest.mock("expo-file-system", () => ({
  File: class {
    async bytes() {
      return mockReadBytes();
    }

    delete() {
      mockDelete();
    }
  },
}));

jest.mock("@/native/runtime-provider", () => ({
  runtimeProvider: {
    availability: { type: "available", platform: "ios" },
    runtime: {
      createGeneratedIdentity: () => mockCreateGeneratedIdentity(),
      createImportedIdentity: (identity: Uint8Array) => mockCreateImportedIdentity(identity),
      previewIdentityImport: (identity: Uint8Array) => mockPreviewIdentityImport(identity),
    },
  },
}));

describe("identity onboarding", () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it("routes a generated identity directly to local-node details", async () => {
    mockCreateGeneratedIdentity.mockResolvedValue({
      type: "created",
      identityHash: new Uint8Array(16).fill(0x11),
    });
    const view = render(<OnboardingScreen step="create" />);

    fireEvent.press(view.getByRole("button", { name: "Create identity" }));
    await waitFor(() =>
      expect(view.getByRole("button", { name: "Continue to local node" })).toBeTruthy(),
    );
    fireEvent.press(view.getByRole("button", { name: "Continue to local node" }));

    expect(mockReplace).toHaveBeenCalledWith("/nodes/local");
  });

  it("previews and imports the same bytes before opening local-node details", async () => {
    const credential = new Uint8Array(64).fill(0x22);
    const identityHash = new Uint8Array(16).fill(0x33);
    mockGetDocumentAsync.mockResolvedValue({
      canceled: false,
      assets: [{ uri: "file:///identity.raw" }],
    });
    mockReadBytes.mockResolvedValue(credential);
    mockPreviewIdentityImport.mockResolvedValue({ type: "valid", identityHash });
    mockCreateImportedIdentity.mockResolvedValue({ type: "created", identityHash });
    const view = render(<OnboardingScreen step="import" />);

    fireEvent.press(view.getByRole("button", { name: "Choose raw credential" }));
    await waitFor(() => expect(view.getByText("33333333333333333333333333333333")).toBeTruthy());
    fireEvent.press(view.getByRole("button", { name: "Confirm and import" }));
    await waitFor(() =>
      expect(view.getByRole("button", { name: "Continue to local node" })).toBeTruthy(),
    );
    fireEvent.press(view.getByRole("button", { name: "Continue to local node" }));

    expect(mockPreviewIdentityImport).toHaveBeenCalledWith(credential);
    expect(mockCreateImportedIdentity).toHaveBeenCalledWith(credential);
    expect(mockGetDocumentAsync).toHaveBeenCalledWith({
      copyToCacheDirectory: true,
      multiple: false,
    });
    expect(mockDelete).toHaveBeenCalledTimes(1);
    expect(mockReplace).toHaveBeenCalledWith("/nodes/local");
  });
});
