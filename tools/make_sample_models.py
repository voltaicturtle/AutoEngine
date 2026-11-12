#!/usr/bin/env python3
"""Generates the sample unit models in content/models.

The models are simple original shapes built from lathed profiles and boxes.
They exist to exercise the client's model loading (OBJ with materials, OBJ
with a texture, PLY with vertex colours, and glTF with a skeleton and
animation clips); replace them with real art.
Models face +Z and stand on y = 0.

Usage: python3 tools/make_sample_models.py [output directory]
"""
import base64
import json
import math
import os
import random
import struct
import sys
import zlib


class Mesh:
    def __init__(self):
        self.v, self.n, self.t = [], [], []
        self.faces = {}  # material -> list of faces, each a list of vertex indices
        self.color = {}  # material -> (r, g, b)

    def add(self, p, n, t=(0.0, 0.0)):
        length = math.sqrt(sum(c * c for c in n)) or 1.0
        self.v.append(p)
        self.n.append(tuple(c / length for c in n))
        self.t.append(t)
        return len(self.v) - 1

    def face(self, material, idx):
        self.faces.setdefault(material, []).append(idx)

    def lathe(self, material, profile, at=(0.0, 0.0, 0.0), segments=14, squash=(1.0, 1.0)):
        """Spins a list of (radius, height) points around the Y axis."""
        rings = []
        for i, (r, y) in enumerate(profile):
            a = profile[max(i - 1, 0)]
            b = profile[min(i + 1, len(profile) - 1)]
            dr, dy = b[0] - a[0], b[1] - a[1]
            ring = []
            for s in range(segments):
                ang = 2 * math.pi * s / segments
                c, sn = math.cos(ang), math.sin(ang)
                p = (at[0] + r * c * squash[0], at[1] + y, at[2] + r * sn * squash[1])
                ring.append(self.add(p, (dy * c, -dr, dy * sn), (s / segments, i / len(profile))))
            rings.append(ring)
        for a, b in zip(rings, rings[1:]):
            for s in range(segments):
                s2 = (s + 1) % segments
                self.face(material, [a[s], b[s], b[s2], a[s2]])

    def box(self, material, center, size, yaw=0.0, pitch=0.0):
        hx, hy, hz = (c / 2 for c in size)
        cy, sy = math.cos(yaw), math.sin(yaw)
        cp, sp = math.cos(pitch), math.sin(pitch)

        def turn(p):
            x, y, z = p
            y, z = y * cp - z * sp, y * sp + z * cp
            x, z = x * cy + z * sy, -x * sy + z * cy
            return (x, y, z)

        sides = [
            ((1, 0, 0), (0, 1, 0), (0, 0, 1)),
            ((-1, 0, 0), (0, 0, 1), (0, 1, 0)),
            ((0, 1, 0), (0, 0, 1), (1, 0, 0)),
            ((0, -1, 0), (1, 0, 0), (0, 0, 1)),
            ((0, 0, 1), (1, 0, 0), (0, 1, 0)),
            ((0, 0, -1), (0, 1, 0), (1, 0, 0)),
        ]
        for n, u, w in sides:
            idx = []
            for a, b in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
                local = tuple((n[k] + a * u[k] + b * w[k]) * (hx, hy, hz)[k] for k in range(3))
                world = tuple(c + o for c, o in zip(turn(local), center))
                idx.append(self.add(world, turn(n), ((a + 1) / 2, (b + 1) / 2)))
            self.face(material, idx)

    def ground(self):
        low = min(p[1] for p in self.v)
        self.v = [(x, y - low, z) for x, y, z in self.v]

    def write_obj(self, path, texture=None):
        name = os.path.splitext(os.path.basename(path))[0]
        with open(path, "w") as f:
            f.write(f"# AutoEngine sample model\nmtllib {name}.mtl\n")
            for p in self.v:
                f.write("v %.4f %.4f %.4f\n" % p)
            for t in self.t:
                f.write("vt %.4f %.4f\n" % t)
            for n in self.n:
                f.write("vn %.4f %.4f %.4f\n" % n)
            for material, faces in self.faces.items():
                f.write(f"usemtl {material}\n")
                for face in faces:
                    f.write("f " + " ".join(f"{i + 1}/{i + 1}/{i + 1}" for i in face) + "\n")
        with open(os.path.splitext(path)[0] + ".mtl", "w") as f:
            for material in self.faces:
                r, g, b = self.color[material]
                f.write(f"newmtl {material}\nKd {r:.3f} {g:.3f} {b:.3f}\n")
                if texture and material in texture:
                    f.write(f"map_Kd {texture[material]}\n")
                f.write("\n")

    def write_ply(self, path):
        """ASCII PLY with a colour on every vertex instead of materials."""
        colors = [(255, 255, 255)] * len(self.v)
        tris = []
        for material, faces in self.faces.items():
            base = self.color[material]
            for face in faces:
                for i in face:
                    # Fade towards white with height for a glow.
                    k = min(max(self.v[i][1], 0.0), 1.0) * 0.6
                    colors[i] = tuple(int(255 * (c + (1 - c) * k)) for c in base)
                tris.append(face)
        with open(path, "w") as f:
            f.write("ply\nformat ascii 1.0\ncomment AutoEngine sample model\n")
            f.write(f"element vertex {len(self.v)}\n")
            f.write("property float x\nproperty float y\nproperty float z\n")
            f.write("property float nx\nproperty float ny\nproperty float nz\n")
            f.write("property uchar red\nproperty uchar green\nproperty uchar blue\n")
            f.write(f"element face {len(tris)}\nproperty list uchar int vertex_indices\nend_header\n")
            for p, n, c in zip(self.v, self.n, colors):
                f.write("%.4f %.4f %.4f %.4f %.4f %.4f %d %d %d\n" % (*p, *n, *c))
            for face in tris:
                f.write(f"{len(face)} " + " ".join(map(str, face)) + "\n")


def ball(r, y, steps=7):
    return [(r * math.sin(math.pi * i / steps), y - r * math.cos(math.pi * i / steps)) for i in range(steps + 1)]


def write_png(path, width, height, pixel):
    rows = b"".join(b"\x00" + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(rows, 9)))
        f.write(chunk(b"IEND", b""))


def warden():
    m = Mesh()
    m.color = {"bark": (0.36, 0.25, 0.16), "leaf": (0.25, 0.55, 0.24), "iron": (0.62, 0.65, 0.70)}
    m.lathe("bark", [(0.0, 0.0), (0.42, 0.0), (0.36, 0.25), (0.40, 0.75), (0.30, 0.95), (0.0, 0.95)])
    m.lathe("leaf", ball(0.34, 1.18))
    m.lathe("leaf", ball(0.20, 1.02), at=(0.30, 0, 0.0))
    m.lathe("leaf", ball(0.20, 1.02), at=(-0.30, 0, 0.0))
    m.box("iron", (0.0, 0.52, 0.46), (0.58, 0.68, 0.08))
    m.box("bark", (0.0, 0.52, 0.51), (0.16, 0.16, 0.05))
    return m


def stormcaller():
    m = Mesh()
    m.color = {"robe": (0.16, 0.40, 0.66), "skin": (0.80, 0.90, 0.95), "orb": (0.55, 0.95, 1.0)}
    m.lathe("robe", [(0.0, 0.0), (0.44, 0.0), (0.30, 0.40), (0.20, 0.85), (0.24, 0.95), (0.0, 0.98)])
    m.lathe("skin", ball(0.17, 1.12))
    m.lathe("robe", [(0.30, 1.20), (0.20, 1.26), (0.0, 1.30)])
    m.lathe("orb", ball(0.13, 0.95), at=(0.0, 0.0, 0.45), segments=10)
    m.box("robe", (0.0, 0.80, 0.25), (0.08, 0.08, 0.35))
    return m


def mender():
    m = Mesh()
    m.color = {"stem": (0.35, 0.62, 0.30), "petal": (0.98, 0.72, 0.80), "heart": (1.0, 0.88, 0.40)}
    m.lathe("stem", [(0.0, 0.0), (0.26, 0.0), (0.12, 0.30), (0.10, 0.85), (0.0, 0.90)])
    for i in range(7):
        a = 2 * math.pi * i / 7
        m.lathe("petal", ball(0.17, 1.0, 5), at=(0.27 * math.cos(a), 0.0, 0.27 * math.sin(a)), segments=8)
    m.lathe("heart", ball(0.20, 1.05))
    m.box("stem", (0.22, 0.42, 0.0), (0.28, 0.04, 0.14), yaw=0.3)
    m.box("stem", (-0.22, 0.30, 0.0), (0.28, 0.04, 0.14), yaw=-0.3)
    return m


def keeper():
    m = Mesh()
    m.color = {"cloak": (0.26, 0.22, 0.34), "dark": (0.06, 0.05, 0.09), "lamp": (1.0, 0.85, 0.35), "frame": (0.30, 0.28, 0.25)}
    m.lathe("cloak", [(0.0, 0.0), (0.36, 0.0), (0.26, 0.55), (0.28, 0.90), (0.22, 1.15), (0.0, 1.32)])
    m.lathe("dark", ball(0.15, 1.02), at=(0.0, 0.0, 0.14), segments=10)
    m.box("frame", (0.36, 0.95, 0.20), (0.04, 0.04, 0.40))
    m.box("frame", (0.36, 0.84, 0.38), (0.02, 0.20, 0.02))
    m.box("lamp", (0.36, 0.66, 0.38), (0.15, 0.20, 0.15))
    m.box("frame", (0.36, 0.78, 0.38), (0.18, 0.03, 0.18))
    return m


def hound():
    m = Mesh()
    m.color = {"fur": (0.30, 0.29, 0.31), "ember": (0.95, 0.42, 0.14), "eye": (1.0, 0.9, 0.4)}
    m.box("fur", (0.0, 0.48, 0.0), (0.34, 0.32, 0.80))
    m.box("fur", (0.0, 0.66, 0.50), (0.28, 0.28, 0.32))
    m.box("fur", (0.0, 0.60, 0.72), (0.16, 0.14, 0.20))
    for x in (-0.11, 0.11):
        m.box("fur", (x, 0.88, 0.42), (0.07, 0.16, 0.07))
        m.box("eye", (x * 0.8, 0.72, 0.665), (0.05, 0.04, 0.02))
        for z in (-0.28, 0.28):
            m.box("fur", (x * 1.1, 0.17, z), (0.10, 0.34, 0.10))
    m.box("ember", (0.0, 0.67, -0.05), (0.10, 0.08, 0.70))
    m.box("ember", (0.0, 0.66, -0.52), (0.08, 0.08, 0.34), pitch=0.6)
    return m


def wisp():
    m = Mesh()
    m.color = {"glow": (0.45, 0.85, 0.80)}
    m.lathe("glow", [(0.0, 0.25), (0.20, 0.38), (0.26, 0.58), (0.18, 0.80), (0.07, 1.00), (0.0, 1.15)], segments=16)
    m.lathe("glow", ball(0.05, 0.12), at=(0.2, 0.0, 0.0), segments=8)
    m.lathe("glow", ball(0.04, 0.2), at=(-0.15, 0.0, 0.14), segments=8)
    return m


def golem():
    m = Mesh()
    m.color = {"moss": (1.0, 1.0, 1.0), "eye": (0.95, 0.85, 0.35)}
    m.box("moss", (0.0, 0.62, 0.0), (0.72, 0.62, 0.50))
    m.box("moss", (0.0, 1.08, 0.04), (0.38, 0.30, 0.36))
    for x in (-1, 1):
        m.box("moss", (x * 0.52, 0.55, 0.06), (0.26, 0.70, 0.30))
        m.box("moss", (x * 0.20, 0.16, 0.0), (0.28, 0.32, 0.34))
        m.box("eye", (x * 0.09, 1.10, 0.225), (0.07, 0.05, 0.02))
    return m


# --- Rigged models (glTF) -------------------------------------------------

# Joint name -> (parent, position in the bind pose).
JOINTS = {
    "hips": (None, (0.0, 0.55, 0.0)),
    "spine": ("hips", (0.0, 0.62, 0.0)),
    "head": ("spine", (0.0, 1.15, 0.0)),
    "arm_r": ("spine", (0.30, 1.06, 0.0)),
    "arm_l": ("spine", (-0.30, 1.06, 0.0)),
    "leg_r": ("hips", (0.12, 0.55, 0.0)),
    "leg_l": ("hips", (-0.12, 0.55, 0.0)),
}


def rot(axis, angle):
    """Quaternion (x, y, z, w) for a turn of `angle` radians around an axis."""
    s = math.sin(angle / 2)
    return tuple(s * a for a in {"x": (1, 0, 0), "y": (0, 1, 0), "z": (0, 0, 1)}[axis]) + (math.cos(angle / 2),)


def keys(axis, *pairs):
    return [(t, rot(axis, a)) for t, a in pairs]


# Clip name -> joint -> rotation keys. Forward is +Z, so a negative turn
# around X swings a hanging arm forward and up.
CLIPS = {
    "idle": {
        "spine": keys("z", (0, 0), (0.5, 0.04), (1.0, 0), (1.5, -0.04), (2.0, 0)),
        "head": keys("y", (0, 0), (0.5, 0.2), (1.0, 0), (1.5, -0.2), (2.0, 0)),
        "arm_r": keys("x", (0, 0), (1.0, -0.12), (2.0, 0)),
        "arm_l": keys("x", (0, 0), (1.0, 0.12), (2.0, 0)),
    },
    "walk": {
        "leg_r": keys("x", (0, 0.6), (0.3, -0.6), (0.6, 0.6)),
        "leg_l": keys("x", (0, -0.6), (0.3, 0.6), (0.6, -0.6)),
        "arm_r": keys("x", (0, -0.5), (0.3, 0.5), (0.6, -0.5)),
        "arm_l": keys("x", (0, 0.5), (0.3, -0.5), (0.6, 0.5)),
        "spine": keys("x", (0, 0.1), (0.6, 0.1)),
    },
    "attack": {
        "arm_r": keys("x", (0, 0), (0.15, -2.7), (0.27, -0.8), (0.45, 0)),
        "spine": keys("x", (0, 0), (0.15, -0.2), (0.27, 0.35), (0.45, 0)),
        "leg_r": keys("x", (0, 0), (0.27, -0.4), (0.45, 0)),
    },
    "cast": {
        "arm_r": keys("x", (0, 0), (0.3, -2.8), (0.7, -2.8), (0.9, 0)),
        "arm_l": keys("x", (0, 0), (0.3, -2.8), (0.7, -2.8), (0.9, 0)),
        "head": keys("x", (0, 0), (0.3, -0.35), (0.7, -0.35), (0.9, 0)),
    },
}


def write_rigged_gltf(path, boxes):
    """A jointed figure. `boxes` is a list of (joint, colour, centre, size)
    or (joint, colour, centre, size, pitch); every box follows one joint."""
    names = list(JOINTS)
    mesh = Mesh()
    joint_of, color_of = [], []
    for joint, color, center, size, *pitch in boxes:
        before = len(mesh.v)
        mesh.box("m", center, size, pitch=pitch[0] if pitch else 0.0)
        joint_of += [names.index(joint)] * (len(mesh.v) - before)
        color_of += [color] * (len(mesh.v) - before)
    indices = []
    for quad in mesh.faces["m"]:
        indices += [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]

    blob = bytearray()
    views, accessors = [], []

    def add(data, component, kind, count, target=None, bounds=None):
        while len(blob) % 4:
            blob.append(0)
        view = {"buffer": 0, "byteOffset": len(blob), "byteLength": len(data)}
        if target:
            view["target"] = target
        blob.extend(data)
        views.append(view)
        accessor = {"bufferView": len(views) - 1, "componentType": component, "count": count, "type": kind}
        if bounds:
            accessor["min"], accessor["max"] = bounds
        accessors.append(accessor)
        return len(accessors) - 1

    def floats(rows):
        flat = [c for row in rows for c in row]
        return struct.pack(f"<{len(flat)}f", *flat)

    n = len(mesh.v)
    low = [min(p[k] for p in mesh.v) for k in range(3)]
    high = [max(p[k] for p in mesh.v) for k in range(3)]
    attributes = {
        "POSITION": add(floats(mesh.v), 5126, "VEC3", n, 34962, (low, high)),
        "NORMAL": add(floats(mesh.n), 5126, "VEC3", n, 34962),
        "COLOR_0": add(floats([c + (1.0,) for c in color_of]), 5126, "VEC4", n, 34962),
        "JOINTS_0": add(bytes(b for j in joint_of for b in (j, 0, 0, 0)), 5121, "VEC4", n, 34962),
        "WEIGHTS_0": add(floats([(1.0, 0.0, 0.0, 0.0)] * n), 5126, "VEC4", n, 34962),
    }
    index_accessor = add(struct.pack(f"<{len(indices)}H", *indices), 5123, "SCALAR", len(indices), 34963)
    # Bind matrices undo each joint's position (column-major, translation last).
    inverse_bind = add(
        floats([(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -x, -y, -z, 1) for _, (x, y, z) in JOINTS.values()]),
        5126, "MAT4", len(names))

    # Node 0 carries the mesh; nodes 1.. are the joints.
    nodes = [{"name": "body", "mesh": 0, "skin": 0}]
    for name, (parent, pos) in JOINTS.items():
        origin = JOINTS[parent][1] if parent else (0.0, 0.0, 0.0)
        node = {"name": name, "translation": [a - b for a, b in zip(pos, origin)]}
        children = [names.index(c) + 1 for c, (p, _) in JOINTS.items() if p == name]
        if children:
            node["children"] = children
        nodes.append(node)

    animations = []
    for clip, tracks in CLIPS.items():
        samplers, channels = [], []
        for joint, track in tracks.items():
            times = [t for t, _ in track]
            time_accessor = add(struct.pack(f"<{len(times)}f", *times), 5126, "SCALAR", len(times), None, ([min(times)], [max(times)]))
            value_accessor = add(floats([q for _, q in track]), 5126, "VEC4", len(track))
            samplers.append({"input": time_accessor, "output": value_accessor, "interpolation": "LINEAR"})
            channels.append({"sampler": len(samplers) - 1, "target": {"node": names.index(joint) + 1, "path": "rotation"}})
        animations.append({"name": clip, "samplers": samplers, "channels": channels})

    gltf = {
        "asset": {"version": "2.0", "generator": "AutoEngine sample models"},
        "scene": 0,
        "scenes": [{"nodes": [0, 1]}],
        "nodes": nodes,
        "meshes": [{"primitives": [{"attributes": attributes, "indices": index_accessor, "material": 0}]}],
        "materials": [{"name": "painted", "pbrMetallicRoughness": {"baseColorFactor": [1, 1, 1, 1], "metallicFactor": 0.0}}],
        "skins": [{"joints": list(range(1, len(names) + 1)), "skeleton": 1, "inverseBindMatrices": inverse_bind}],
        "animations": animations,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": len(blob), "uri": "data:application/octet-stream;base64," + base64.b64encode(bytes(blob)).decode()}],
    }
    with open(path, "w") as f:
        json.dump(gltf, f, separators=(",", ":"))


def figure(cloth, skin, trim, weapon):
    return [
        ("hips", cloth, (0.0, 0.58, 0.0), (0.36, 0.16, 0.22)),
        ("spine", cloth, (0.0, 0.90, 0.0), (0.42, 0.50, 0.24)),
        ("spine", trim, (0.0, 0.92, 0.125), (0.14, 0.34, 0.02)),
        ("head", skin, (0.0, 1.30, 0.0), (0.26, 0.26, 0.26)),
        ("head", trim, (0.0, 1.46, -0.02), (0.30, 0.08, 0.30)),
        ("head", (0.08, 0.08, 0.10), (0.07, 1.32, 0.135), (0.05, 0.05, 0.02)),
        ("head", (0.08, 0.08, 0.10), (-0.07, 1.32, 0.135), (0.05, 0.05, 0.02)),
        ("arm_r", skin, (0.30, 0.82, 0.0), (0.12, 0.52, 0.12)),
        ("arm_l", skin, (-0.30, 0.82, 0.0), (0.12, 0.52, 0.12)),
        ("leg_r", trim, (0.12, 0.28, 0.0), (0.14, 0.56, 0.14)),
        ("leg_l", trim, (-0.12, 0.28, 0.0), (0.14, 0.56, 0.14)),
    ] + weapon


def moss_pixel(rng):
    cells = [[rng.random() for _ in range(64)] for _ in range(64)]

    def pixel(x, y):
        n = cells[y][x] * 0.5 + cells[y // 4 * 4][x // 4 * 4] * 0.5
        if n > 0.55:
            return (int(60 + 50 * n), int(120 + 60 * n), int(50 + 30 * n))
        return (int(95 + 50 * n), int(92 + 45 * n), int(88 + 40 * n))

    return pixel



# --- Item icons -----------------------------------------------------------

def write_icon(path, base, glyph):
    """A 64x64 badge: a dark rounded square with a simple shape on it."""
    def pixel(x, y):
        dx, dy = x - 31.5, y - 31.5
        edge = max(abs(dx), abs(dy))
        if edge > 30 or (abs(dx) > 26 and abs(dy) > 26 and (abs(dx) - 26) ** 2 + (abs(dy) - 26) ** 2 > 16):
            return (18, 20, 28)
        if glyph(dx, dy):
            return tuple(min(255, int(c * 255 * (1.15 - (y / 64) * 0.4))) for c in base)
        shade = 0.22 + 0.10 * (1 - y / 64)
        return tuple(int(c * 255 * shade) for c in base)
    write_png(path, 64, 64, pixel)


ICONS = {
    "whetstone": ((0.70, 0.74, 0.80), lambda x, y: abs(x + y) < 7 and abs(x - y) < 34),
    "oak_charm": ((0.45, 0.75, 0.35), lambda x, y: x * x + (y + 4) ** 2 < 260 or (abs(x) < 4 and 8 < y < 22)),
    "ember_core": ((1.00, 0.50, 0.20), lambda x, y: x * x + y * y < 190 and not x * x + y * y < 45),
    "barkplate": ((0.60, 0.42, 0.25), lambda x, y: abs(x) < 17 and -18 < y < 14 - abs(x) * 0.5),
    "cinder_blade": ((1.00, 0.72, 0.25), lambda x, y: abs(x - y) < 5 and abs(x + y) < 36 or (abs(x + y + 22) < 4 and abs(x - y) < 16)),
    "grove_sigil": ((0.35, 0.85, 0.55), lambda x, y: abs(abs(x) + abs(y) - 16) < 4 or x * x + y * y < 20),
}

def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "content", "models")
    os.makedirs(out, exist_ok=True)
    models = {
        "ironbark_warden": warden,
        "tidewater_stormcaller": stormcaller,
        "dawnpetal_mender": mender,
        "hollow_lantern_keeper": keeper,
        "ashen_hound": hound,
    }
    for name, build in models.items():
        mesh = build()
        mesh.ground()
        mesh.write_obj(os.path.join(out, name + ".obj"))
    mesh = golem()
    mesh.ground()
    write_png(os.path.join(out, "moss_golem.png"), 64, 64, moss_pixel(random.Random(7)))
    mesh.write_obj(os.path.join(out, "moss_golem.obj"), texture={"moss": "moss_golem.png"})
    mesh = wisp()
    mesh.write_ply(os.path.join(out, "grave_wisp.ply"))

    icons = os.path.join(out, "..", "icons")
    os.makedirs(icons, exist_ok=True)
    for name, (base, glyph) in ICONS.items():
        write_icon(os.path.join(icons, name + ".png"), base, glyph)

    blade = (1.0, 0.72, 0.25)
    write_rigged_gltf(
        os.path.join(out, "emberlash_duelist.gltf"),
        figure((0.72, 0.20, 0.14), (0.93, 0.74, 0.58), (0.30, 0.10, 0.10), [
            ("arm_r", blade, (0.30, 0.56, 0.36), (0.05, 0.05, 0.70)),
            ("arm_r", (0.30, 0.10, 0.10), (0.30, 0.56, 0.04), (0.18, 0.05, 0.05)),
        ]),
    )
    shaft = (0.85, 0.82, 0.70)
    write_rigged_gltf(
        os.path.join(out, "reed_stalker.gltf"),
        figure((0.50, 0.58, 0.28), (0.72, 0.80, 0.55), (0.30, 0.36, 0.16), [
            ("arm_r", shaft, (0.30, 0.56, 0.25), (0.04, 0.04, 1.50)),
            ("arm_r", (0.95, 0.95, 0.90), (0.30, 0.56, 1.05), (0.10, 0.03, 0.22)),
        ]),
    )


if __name__ == "__main__":
    main()
