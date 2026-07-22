import json
import sys

import RNS


def main():
    request = json.load(sys.stdin)
    identity = RNS.Identity.from_bytes(bytes.fromhex(request["secret"]))
    wrong_identity = RNS.Identity.from_bytes(bytes.fromhex(request["wrong_secret"]))
    names = []
    for name in request["names"]:
        names.append(
            {
                "name_hash": RNS.Identity.full_hash(name.encode())[:10].hex(),
                "destination_hash": RNS.Destination.hash_from_name_and_identity(
                    name, identity
                ).hex(),
            }
        )
    cases = []
    for case in request["cases"]:
        message = bytes.fromhex(case["message"])
        signature = identity.sign(message)
        corrupted = bytearray(signature)
        corrupted[len(corrupted) // 2] ^= 1
        rust_token = bytes.fromhex(case["rust_token"])
        rust_plaintext = identity.decrypt(rust_token)
        cases.append(
            {
                "signature": signature.hex(),
                "valid": identity.validate(signature, message),
                "corrupted_valid": identity.validate(bytes(corrupted), message),
                "wrong_valid": wrong_identity.validate(signature, message),
                "rust_plaintext": rust_plaintext.hex() if rust_plaintext is not None else None,
                "rust_mutations_rejected": [
                    identity.decrypt(bytes.fromhex(mutation)) is None
                    for mutation in case["mutations"]
                ],
                "python_token": identity.encrypt(message).hex(),
            }
        )
    json.dump(
        {
            "version": RNS.__version__,
            "public": identity.get_public_key().hex(),
            "identity_hash": identity.hash.hex(),
            "names": names,
            "cases": cases,
        },
        sys.stdout,
    )


if __name__ == "__main__":
    main()
