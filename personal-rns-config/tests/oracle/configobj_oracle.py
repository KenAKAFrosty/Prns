import sys
import io
import json
from RNS.vendor import configobj


def canon(section):
    out = {}
    for key, value in section.items():
        if isinstance(value, dict):
            out[key] = canon(value)
        elif isinstance(value, list):
            out[key] = [str(item) for item in value]
        else:
            out[key] = str(value)
    return out


def main():
    configs = json.load(sys.stdin)
    results = []
    for text in configs:
        try:
            parsed = configobj.ConfigObj(io.StringIO(text))
            results.append({"ok": canon(parsed)})
        except Exception as exc:
            results.append({"error": str(exc)})
    json.dump(results, sys.stdout)


if __name__ == "__main__":
    main()
