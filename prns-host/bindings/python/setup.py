import os
import re

from setuptools import Distribution, setup
from setuptools.command.bdist_wheel import bdist_wheel

PLATFORM_TAG = re.compile(r"^[A-Za-z0-9_.-]+$")


class BinaryDistribution(Distribution):
    def has_ext_modules(self):
        return True


class NativeWheel(bdist_wheel):
    def finalize_options(self):
        platform_tag = os.environ.get("PRNS_WHEEL_PLATFORM_TAG")
        if platform_tag:
            if not PLATFORM_TAG.fullmatch(platform_tag):
                raise ValueError(
                    "PRNS_WHEEL_PLATFORM_TAG contains invalid characters"
                )
            self.plat_name = platform_tag
        super().finalize_options()

    def get_tag(self):
        _, _, platform_tag = super().get_tag()
        return "py3", "none", platform_tag


setup(
    cmdclass={"bdist_wheel": NativeWheel},
    distclass=BinaryDistribution,
)
