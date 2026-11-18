#!/usr/bin/env python3
"""Select locked registry libraries with no foreign build or macro dependencies."""
import functools
import json
import sys

metadata = json.load(sys.stdin)
packages = {package['id']: package for package in metadata['packages']}
nodes = {node['id']: node for node in metadata['resolve']['nodes']}
registry = 'registry+https://github.com/rust-lang/crates.io-index'
foreign_targets = {'custom-build', 'proc-macro'}


@functools.cache
def eligible(package_id):
    package = packages[package_id]
    if package['source'] != registry:
        return False
    if any(foreign_targets.intersection(target['kind']) for target in package['targets']):
        return False
    return all(
        eligible(dependency['pkg'])
        for dependency in nodes[package_id]['deps']
        if any(kind['kind'] != 'dev' for kind in dependency['dep_kinds'])
    )


for name in sorted({
    f"{package['name']}-{package['version']}"
    for package in packages.values() if eligible(package['id'])
}):
    print(name)
