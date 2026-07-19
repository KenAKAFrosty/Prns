import base64
import pathlib
import sys

import RNS

EXPECTED_RNS_VERSION = "1.3.8"
ENCRYPTION_CHUNK_LEN = 1024 * 1024 * RNS.Identity.AES256_BLOCKSIZE
DECRYPTION_CHUNK_LEN = ENCRYPTION_CHUNK_LEN + RNS.Cryptography.Token.TOKEN_OVERHEAD * 2
PRIVATE = bytes([0x22]) * 32 + bytes([0x11]) * 32


def require_reference_version():
    version = getattr(RNS, "__version__", "")
    if version != EXPECTED_RNS_VERSION:
        raise RuntimeError(f"expected RNS {EXPECTED_RNS_VERSION}, got {version!r}")


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


def main():
    require_reference_version()
    command, *arguments = sys.argv[1:]
    if command == "prepare" and len(arguments) == 6:
        prepare(*arguments)
    elif command == "verify" and len(arguments) == 3:
        verify(*arguments)
    elif command == "decrypt" and len(arguments) == 3:
        decrypt(*arguments)
    elif command == "encoding" and len(arguments) == 2:
        encoding(*arguments)
    else:
        raise RuntimeError("invalid oracle command")


if __name__ == "__main__":
    main()
