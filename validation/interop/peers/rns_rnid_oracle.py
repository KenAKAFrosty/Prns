import base64
import pathlib
import sys

import RNS
from RNS.Utilities.rnid import create_rsg, validate_rsg

ENCRYPTION_CHUNK_LEN = 1024 * 1024 * RNS.Identity.AES256_BLOCKSIZE
DECRYPTION_CHUNK_LEN = ENCRYPTION_CHUNK_LEN + RNS.Cryptography.Token.TOKEN_OVERHEAD * 2
PRIVATE = bytes([0x22]) * 32 + bytes([0x11]) * 32


def prepare(private_path, public_path, message_path, signature_path, plaintext_path, encrypted_path):
    identity = RNS.Identity.from_bytes(PRIVATE)
    identity.to_file(private_path)
    pathlib.Path(public_path).write_bytes(identity.get_public_key())
    message = b"local-id-oracle"
    pathlib.Path(message_path).write_bytes(message)
    pathlib.Path(signature_path).write_bytes(identity.sign(message))
    pattern = bytes(range(251))
    plaintext = (pattern * ((ENCRYPTION_CHUNK_LEN + 37) // len(pattern) + 1))[
        : ENCRYPTION_CHUNK_LEN + 37
    ]
    pathlib.Path(plaintext_path).write_bytes(plaintext)
    with pathlib.Path(encrypted_path).open("wb") as output:
        for offset in range(0, len(plaintext), ENCRYPTION_CHUNK_LEN):
            output.write(identity.encrypt(plaintext[offset : offset + ENCRYPTION_CHUNK_LEN]))
    print(identity.hash.hex())


def verify(private_path, message_path, signature_path):
    identity = RNS.Identity.from_file(private_path)
    message = pathlib.Path(message_path).read_bytes()
    signature = pathlib.Path(signature_path).read_bytes()
    if not identity.validate(signature, message):
        raise RuntimeError("stock RNS rejected the raw Prns signature")


def decrypt(private_path, encrypted_path, expected_path):
    identity = RNS.Identity.from_file(private_path)
    plaintext = bytearray()
    with pathlib.Path(encrypted_path).open("rb") as encrypted:
        while chunk := encrypted.read(DECRYPTION_CHUNK_LEN):
            opened = identity.decrypt(chunk)
            if opened is None:
                raise RuntimeError("stock RNS could not decrypt the Prns ciphertext")
            plaintext.extend(opened)
    if bytes(plaintext) != pathlib.Path(expected_path).read_bytes():
        raise RuntimeError("stock RNS decrypted different plaintext")


def encoding(public_path, name):
    public = pathlib.Path(public_path).read_bytes()
    values = {
        "hex": public.hex(),
        "base32": base64.b32encode(public).decode("ascii"),
        "base64": base64.urlsafe_b64encode(public).decode("ascii"),
        "base256": RNS.b256rep(public),
    }
    print(values[name])


def prepare_artifacts(private_path, file_path, rsg_path, rsm_path):
    identity = RNS.Identity.from_file(private_path)
    file_message = b"canonical-file-oracle"
    embedded_message = b"canonical-message-oracle"
    pathlib.Path(file_path).write_bytes(file_message)
    pathlib.Path(rsg_path).write_bytes(create_rsg(identity, file_message))
    pathlib.Path(rsm_path).write_bytes(
        create_rsg(
            identity,
            embedded_message,
            embed=True,
            meta={
                "name": "Prns",
                "version": 3,
                "tags": ["one", "two"],
                "stable": True,
            },
        )
    )


def verify_rsg(private_path, file_path, rsg_path):
    identity = RNS.Identity.from_file(private_path)
    valid, signed_data, signer = validate_rsg(
        pathlib.Path(rsg_path).read_bytes(),
        message=pathlib.Path(file_path).read_bytes(),
        required_signer=identity,
    )
    if not valid or signer.hash != identity.hash or "message" in signed_data:
        raise RuntimeError("stock RNS rejected the canonical Prns RSG")


def verify_rsm(private_path, rsm_path):
    identity = RNS.Identity.from_file(private_path)
    rsm = pathlib.Path(rsm_path).read_bytes()
    from RNS.Utilities.rnid import extract_signed_rsg_data

    signed_data = extract_signed_rsg_data(rsm)
    valid, validated, signer = validate_rsg(
        rsm,
        message=signed_data["message"],
        required_signer=identity,
    )
    expected = {
        "name": "Prns",
        "version": 3,
        "tags": ["one", "two"],
        "stable": True,
    }
    actual = {
        key: value
        for key, value in validated["meta"].items()
        if key not in ["signer", "pubkey"]
    }
    if not valid or signer.hash != identity.hash or actual != expected:
        raise RuntimeError("stock RNS rejected the canonical Prns RSM metadata")


def verify_encoded_rsg(private_path, file_path, output_path, encoding):
    lines = pathlib.Path(output_path).read_text(encoding="utf-8").splitlines()
    inside = False
    chunks = []
    for line in lines:
        if line.startswith("#### Start of rsg data "):
            inside = True
        elif inside and line.endswith(" End of rsg data ####"):
            break
        elif inside:
            chunks.append(line)
    encoded = "".join(chunks).rstrip("=")
    if encoding == "hex":
        artifact = bytes.fromhex(encoded)
    elif encoding == "base32":
        artifact = base64.b32decode(encoded + "=" * ((8 - len(encoded) % 8) % 8))
    elif encoding == "base64":
        artifact = base64.urlsafe_b64decode(
            encoded + "=" * ((4 - len(encoded) % 4) % 4)
        )
    elif encoding == "base256":
        artifact = RNS.b256_to_bytes(encoded)
    else:
        raise RuntimeError(f"unknown artifact encoding {encoding}")
    identity = RNS.Identity.from_file(private_path)
    valid, _, _ = validate_rsg(
        artifact,
        message=pathlib.Path(file_path).read_bytes(),
        required_signer=identity,
    )
    if not valid:
        raise RuntimeError(f"stock RNS rejected the {encoding} Prns RSG output")


def main():
    command, *arguments = sys.argv[1:]
    if command == "prepare" and len(arguments) == 6:
        prepare(*arguments)
    elif command == "verify" and len(arguments) == 3:
        verify(*arguments)
    elif command == "decrypt" and len(arguments) == 3:
        decrypt(*arguments)
    elif command == "encoding" and len(arguments) == 2:
        encoding(*arguments)
    elif command == "prepare-artifacts" and len(arguments) == 4:
        prepare_artifacts(*arguments)
    elif command == "verify-rsg" and len(arguments) == 3:
        verify_rsg(*arguments)
    elif command == "verify-rsm" and len(arguments) == 2:
        verify_rsm(*arguments)
    elif command == "verify-encoded-rsg" and len(arguments) == 4:
        verify_encoded_rsg(*arguments)
    else:
        raise RuntimeError("invalid oracle command")


if __name__ == "__main__":
    main()
