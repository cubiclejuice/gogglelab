// Three.js viewport: single-model viewing plus a slicer-style overview of a
// multi-plate project. Geometry remains non-indexed so each triangle keeps its
// own normal for the flat-faceted print-preview look.

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { ColorTarget, ColorwayMode, ResolveColor } from "./color-types";
import type { DecodedGeometry, GeometryStats, PartSummary } from "./ipc";
import { layoutPlates, plateCameraDistance } from "./plate-layout";
import { planGroups } from "./viewport-groups";
import { getEffectiveTheme } from "./theme";

const DIMENSION_FLOOR_MM = 0.001;
const DEFAULT_BED_MM: [number, number, number] = [256, 256, 256];
const SELECTED_MODEL_COLOR = 0x5ac8a0;

export interface ViewportPlate {
  id: number;
  name: string;
  geometry: DecodedGeometry;
  stats: GeometryStats | null;
  parts: PartSummary[];
}

export interface ColorwayRenderState {
  mode: ColorwayMode;
  resolveColor: ResolveColor;
}

interface RenderedMesh {
  mesh: THREE.Mesh<THREE.BufferGeometry, THREE.MeshStandardMaterial | THREE.MeshStandardMaterial[]>;
  materials: THREE.MeshStandardMaterial[];
}

interface RenderedPlate {
  id: number;
  input: ViewportPlate;
  outline: THREE.LineSegments<THREE.BufferGeometry, THREE.LineBasicMaterial>;
  render: RenderedMesh | null;
}

function disposeObject(root: THREE.Object3D) {
  const geometries = new Set<THREE.BufferGeometry>();
  const materials = new Set<THREE.Material>();
  const textures = new Set<THREE.Texture>();

  root.traverse((object) => {
    const renderable = object as THREE.Object3D & {
      geometry?: THREE.BufferGeometry;
      material?: THREE.Material | THREE.Material[];
    };
    if (renderable.geometry) geometries.add(renderable.geometry);
    for (const material of renderable.material
      ? Array.isArray(renderable.material)
        ? renderable.material
        : [renderable.material]
      : []) {
      materials.add(material);
      if (material instanceof THREE.SpriteMaterial && material.map) textures.add(material.map);
    }
  });
  geometries.forEach((geometry) => geometry.dispose());
  materials.forEach((material) => material.dispose());
  textures.forEach((texture) => texture.dispose());
}
export class Viewport {
  public onPlateSelect: (id: number) => void = () => {};

  private renderer: THREE.WebGLRenderer;
  private scene: THREE.Scene;
  private camera: THREE.PerspectiveCamera;
  private controls: OrbitControls;
  private mesh: RenderedMesh | null = null;
  private bedGroup: THREE.Group;
  private overviewGroup: THREE.Group | null = null;
  private overviewPlates: RenderedPlate[] = [];
  private overviewInput: ViewportPlate[] = [];
  private meshInput: ViewportPlate | null = null;
  private overviewMode = false;
  private selectedPlateId: number | null = null;
  private bedMm: [number, number, number] = DEFAULT_BED_MM;
  private lastStats: GeometryStats | null = null;
  private frameBounds: { center: THREE.Vector3; dimensions: [number, number, number] } | null =
    null;
  private dark: boolean;
  private useAccentModelColor = true;
  private colorway: ColorwayRenderState = {
    mode: "model",
    resolveColor: () => ({ color: null, source: "GoggleLab default" }),
  };
  private hemi: THREE.HemisphereLight;
  private axes: THREE.AxesHelper;
  private container: HTMLElement;
  private resizeObserver: ResizeObserver;
  private pointerDown: { x: number; y: number } | null = null;

  private onThemeChange = () => {
    this.dark = getEffectiveTheme() === "dark";
    this.refreshColors();
  };

  private onAccentChange = () => {
    this.refreshColors();
  };

  private onPointerDown = (event: PointerEvent) => {
    this.pointerDown = { x: event.clientX, y: event.clientY };
  };

  private onPointerCancel = () => {
    this.pointerDown = null;
  };

  private onPointerUp = (event: PointerEvent) => {
    const down = this.pointerDown;
    this.pointerDown = null;
    if (!down || Math.hypot(event.clientX - down.x, event.clientY - down.y) > 4) return;
    const id = this.plateAtPointer(event);
    if (id === null) return;
    this.setSelectedPlate(id);
    this.onPlateSelect(id);
  };

  constructor(container: HTMLElement) {
    this.dark = getEffectiveTheme() === "dark";
    this.container = container;

    this.renderer = new THREE.WebGLRenderer({ antialias: true });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    this.renderer.setClearColor(this.readColor("--viewport", "#e8e8e8"));
    this.renderer.domElement.className = "viewport-canvas";
    this.renderer.domElement.tabIndex = 0;
    this.renderer.domElement.setAttribute("aria-label", "3D model viewport");
    container.appendChild(this.renderer.domElement);

    this.scene = new THREE.Scene();
    this.camera = new THREE.PerspectiveCamera(45, 1, 0.1, 100000);
    this.camera.position.set(150, -220, 180);
    this.camera.up.set(0, 0, 1);

    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.target.set(0, 0, 0);

    this.hemi = new THREE.HemisphereLight(
      this.readColor("--viewport-light", "#ffffff"),
      this.readColor("--viewport-ground", "#222222"),
      1.2,
    );
    this.scene.add(this.hemi);
    const dir = new THREE.DirectionalLight(0xffffff, 1.0);
    dir.position.set(100, -150, 250);
    this.scene.add(dir);

    this.bedGroup = new THREE.Group();
    this.scene.add(this.bedGroup);
    this.addBedPlane(DEFAULT_BED_MM);

    this.axes = new THREE.AxesHelper(20);
    (this.axes.material as THREE.Material).depthTest = false;
    this.axes.renderOrder = 10;
    this.scene.add(this.axes);

    this.renderer.domElement.addEventListener("pointerdown", this.onPointerDown);
    this.renderer.domElement.addEventListener("pointerup", this.onPointerUp);
    this.renderer.domElement.addEventListener("pointercancel", this.onPointerCancel);
    this.renderer.domElement.addEventListener("keydown", (e) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "t" || e.key === "f" || e.key === "i") {
        e.preventDefault();
        this.setPresetView(e.key);
        return;
      }
      if (
        e.key === "ArrowLeft" ||
        e.key === "ArrowRight" ||
        e.key === "ArrowUp" ||
        e.key === "ArrowDown"
      ) {
        e.preventDefault();
        this.orbitBy(e.key);
      }
    });

    // Theme and accent changes use stable handlers so dispose can remove them.
    window.addEventListener("gogglelab-theme-change", this.onThemeChange);
    window.addEventListener("gogglelab-accent-change", this.onAccentChange);

    this.resize();
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(container);
    this.renderer.setAnimationLoop(() => {
      this.controls.update();
      this.renderer.render(this.scene, this.camera);
    });
  }
  refreshColors() {
    this.renderer.setClearColor(this.readColor("--viewport", "#e8e8e8"));
    this.hemi.color.copy(this.readColor("--viewport-light", "#ffffff"));
    this.hemi.groundColor.copy(this.readColor("--viewport-ground", "#222222"));
    if (this.overviewMode) {
      this.rebuildOverview(false);
      return;
    }
    this.addBedPlane(this.bedMm);
    if (this.meshInput) this.rebuildMesh(this.meshInput, false);
  }

  setUseAccentModelColor(enabled: boolean) {
    this.useAccentModelColor = enabled;
    if (this.colorway.mode === "model") this.refreshColors();
  }

  setColorway(mode: ColorwayMode, resolveColor: ResolveColor) {
    const previousMode = this.colorway.mode;
    this.colorway = {
      mode,
      resolveColor: typeof resolveColor === "function" ? resolveColor : this.colorway.resolveColor,
    };
    const update = previousMode === mode ? this.updateMeshColors : this.updateMeshMaterials;
    if (this.overviewMode) {
      for (const plate of this.overviewPlates) {
        if (plate.render) {
          update.call(this, plate.render, plate.input, plate.id === this.selectedPlateId);
        }
      }
    } else if (this.mesh && this.meshInput) {
      update.call(this, this.mesh, this.meshInput, true);
    }
  }

  private modelColor(selected: boolean): THREE.Color {
    if (this.useAccentModelColor) return this.readColor("--accent-fill", "#2f5f4c");
    return new THREE.Color(selected ? SELECTED_MODEL_COLOR : this.unselectedModelColor());
  }

  private resolvedColor(target: ColorTarget, fallback: THREE.Color): THREE.Color {
    try {
      const color = this.colorway.resolveColor(target)?.color;
      if (typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color)) return new THREE.Color(color);
    } catch {
      // A bad resolver must not prevent the model from rendering.
    }
    return fallback;
  }

  private materialColor(target: ColorTarget, selected: boolean): THREE.Color {
    if (this.colorway.mode !== "color") return this.modelColor(selected);
    const fallback = this.modelColor(false);
    return this.resolvedColor(target, fallback);
  }

  private readColor(token: string, fallback: string): THREE.Color {
    const value = getComputedStyle(document.documentElement).getPropertyValue(token).trim();
    return new THREE.Color(value || fallback);
  }

  private orbitBy(key: string) {
    const offset = this.camera.position.clone().sub(this.controls.target);
    const angle = Math.PI / 18;
    if (key === "ArrowLeft" || key === "ArrowRight") {
      offset.applyAxisAngle(new THREE.Vector3(0, 0, 1), key === "ArrowLeft" ? angle : -angle);
    } else {
      const right = new THREE.Vector3().crossVectors(offset, this.camera.up).normalize();
      offset.applyAxisAngle(right, key === "ArrowUp" ? angle : -angle);
    }
    this.camera.position.copy(this.controls.target).add(offset);
    this.camera.lookAt(this.controls.target);
    this.controls.update();
  }

  private setPresetView(key: "t" | "f" | "i") {
    const target = this.controls.target;
    const distance = this.camera.position.distanceTo(target) || 300;
    const direction =
      key === "t"
        ? new THREE.Vector3(0, 0, 1)
        : key === "f"
          ? new THREE.Vector3(0, -1, 0.15).normalize()
          : new THREE.Vector3(0.6, -0.9, 0.7).normalize();
    this.camera.position.copy(target).addScaledVector(direction, distance);
    this.camera.lookAt(target);
    this.controls.update();
  }

  private clearGroup(group: THREE.Group) {
    while (group.children.length > 0) {
      const child = group.children[0]!;
      group.remove(child);
      disposeObject(child);
    }
  }

  private addBedPlane(bedMm: [number, number, number]) {
    this.clearGroup(this.bedGroup);
    const [x, y] = bedMm;
    const grid = new THREE.GridHelper(
      Math.max(x, y),
      Math.max(1, Math.round(Math.max(x, y) / 10)),
      this.readColor("--grid-major", this.dark ? "#555555" : "#aaaaaa"),
      this.readColor("--grid-minor", this.dark ? "#333333" : "#cccccc"),
    );
    grid.rotation.x = Math.PI / 2;
    grid.scale.set(x / Math.max(x, y), 1, y / Math.max(x, y));
    grid.position.set(x / 2, y / 2, 0);
    this.bedGroup.add(grid);
  }

  setBed(bedMm: [number, number, number]) {
    this.bedMm = bedMm;
    if (this.overviewMode) {
      this.rebuildOverview(true);
      return;
    }
    this.addBedPlane(bedMm);
    if (this.mesh && this.lastStats) {
      this.placeOnBed(this.mesh.mesh, this.lastStats, [bedMm[0] / 2, bedMm[1] / 2]);
      this.frameCamera(this.lastStats);
    }
  }

  private placeOnBed(mesh: THREE.Mesh, stats: GeometryStats, center: readonly [number, number]) {
    const cx = (stats.bbox.min[0] + stats.bbox.max[0]) / 2;
    const cy = (stats.bbox.min[1] + stats.bbox.max[1]) / 2;
    mesh.position.set(center[0] - cx, center[1] - cy, -stats.bbox.min[2]);
  }

  private resize() {
    const { clientWidth: w, clientHeight: h } = this.container;
    if (w === 0 || h === 0) return;
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(w, h);
    if (this.frameBounds)
      this.frameDimensions(this.frameBounds.center, this.frameBounds.dimensions, true);
  }

  private clearMesh() {
    if (this.mesh) {
      this.scene.remove(this.mesh.mesh);
      disposeObject(this.mesh.mesh);
      this.mesh = null;
    }
    this.lastStats = null;
  }

  private clearOverview(keepInput = false) {
    if (this.overviewGroup) {
      this.scene.remove(this.overviewGroup);
      disposeObject(this.overviewGroup);
      this.overviewGroup = null;
    }
    this.overviewPlates = [];
    if (!keepInput) {
      this.overviewInput = [];
      this.selectedPlateId = null;
    }
  }

  private updateMeshMaterials(render: RenderedMesh, input: ViewportPlate, selected: boolean) {
    const groups =
      this.colorway.mode === "color" ? planGroups(input.geometry.triangleCount, input.parts) : null;
    let materials: THREE.MeshStandardMaterial[];
    if (groups && groups.length > 0) {
      const modelFallback = this.materialColor({ kind: "model" }, false);
      materials = groups.map((group) => {
        const color = this.resolvedColor({ kind: "part", id: group.partId }, modelFallback);
        return new THREE.MeshStandardMaterial({
          color,
          flatShading: true,
          side: THREE.DoubleSide,
        });
      });
    } else {
      materials = [
        new THREE.MeshStandardMaterial({
          color: this.materialColor({ kind: "model" }, selected),
          flatShading: true,
          side: THREE.DoubleSide,
        }),
      ];
    }

    const geometry = render.mesh.geometry;
    geometry.clearGroups();
    if (groups && groups.length > 0) {
      groups.forEach((group, index) =>
        geometry.addGroup(group.startVertex, group.vertexCount, index),
      );
    }
    const oldMaterials = render.materials;
    render.materials = materials;
    render.mesh.material = groups && groups.length > 0 ? materials : materials[0]!;
    oldMaterials.forEach((material) => material.dispose());
  }

  private updateMeshColors(render: RenderedMesh, input: ViewportPlate, selected: boolean) {
    const groups =
      this.colorway.mode === "color" ? planGroups(input.geometry.triangleCount, input.parts) : null;
    if (groups && groups.length > 0 && groups.length === render.materials.length) {
      const modelFallback = this.materialColor({ kind: "model" }, false);
      groups.forEach((group, index) => {
        render.materials[index]!.color.copy(
          this.resolvedColor({ kind: "part", id: group.partId }, modelFallback),
        );
      });
      return;
    }

    const modelColor = this.materialColor({ kind: "model" }, selected);
    render.materials.forEach((material) => material.color.copy(modelColor));
  }

  private buildMesh(input: ViewportPlate, selected: boolean): RenderedMesh | null {
    if (input.geometry.triangleCount <= 0 || !input.stats) return null;

    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(input.geometry.positions, 3));
    geometry.computeVertexNormals();

    const render: RenderedMesh = {
      mesh: new THREE.Mesh(geometry, []),
      materials: [],
    };
    this.updateMeshMaterials(render, input, selected);
    return render;
  }

  private rebuildMesh(input: ViewportPlate | null, frame: boolean) {
    this.bedGroup.visible = true;
    this.addBedPlane(this.bedMm);
    this.clearMesh();
    if (!input || input.geometry.triangleCount <= 0 || !input.stats) {
      if (frame) {
        this.frameDimensions(new THREE.Vector3(this.bedMm[0] / 2, this.bedMm[1] / 2, 0), [
          this.bedMm[0],
          this.bedMm[1],
          1,
        ]);
      }
      return;
    }

    const render = this.buildMesh(input, true);
    if (!render) return;
    this.mesh = render;
    this.scene.add(render.mesh);
    this.lastStats = input.stats;
    this.placeOnBed(render.mesh, input.stats, [this.bedMm[0] / 2, this.bedMm[1] / 2]);
    if (frame) this.frameCamera(input.stats);
  }

  /** Replaces the display with one centered model and exits plate overview. */
  loadMesh(input: ViewportPlate | null) {
    this.clearOverview();
    this.overviewMode = false;
    this.meshInput = input;
    this.rebuildMesh(input, true);
  }

  /** Displays every project plate together. Selecting a plate does not move the camera. */
  loadPlates(inputs: readonly ViewportPlate[], selectedId: number) {
    this.clearMesh();
    this.clearOverview();
    this.overviewMode = true;
    this.meshInput = null;
    this.bedGroup.visible = false;
    this.overviewInput = [...inputs];
    this.selectedPlateId = inputs.some((plate) => plate.id === selectedId)
      ? selectedId
      : (inputs[0]?.id ?? null);
    this.buildOverview(true);
  }

  setSelectedPlate(id: number) {
    this.selectedPlateId = id;
    for (const plate of this.overviewPlates) {
      const selected = plate.id === id;
      plate.outline.material.color.setHex(
        selected ? SELECTED_MODEL_COLOR : this.gridOutlineColor(),
      );
      if (plate.render && this.colorway.mode === "model") {
        for (const material of plate.render.materials) {
          material.color.copy(this.modelColor(selected));
        }
      }
    }
  }

  private buildOverview(frame: boolean) {
    const layout = layoutPlates(this.overviewInput, this.bedMm);
    const group = new THREE.Group();
    this.overviewGroup = group;
    this.scene.add(group);
    const tilesById = new Map(layout.tiles.map((tile) => [tile.id, tile]));
    let maxHeight = 0;

    for (const plate of this.overviewInput) {
      const tile = tilesById.get(plate.id);
      if (!tile) continue;
      const plateGroup = this.createPlateVisual(plate, tile.center);
      group.add(plateGroup.group);
      this.overviewPlates.push(plateGroup.rendered);
      maxHeight = Math.max(maxHeight, plate.stats?.dimensions[2] ?? 0);
    }
    this.setSelectedPlate(this.selectedPlateId ?? -1);
    if (frame && layout.tiles.length > 0) {
      this.frameDimensions(new THREE.Vector3(0, 0, Math.max(maxHeight, DIMENSION_FLOOR_MM) / 2), [
        layout.bounds.dimensions[0],
        layout.bounds.dimensions[1],
        Math.max(maxHeight, 1),
      ]);
    }
  }

  private rebuildOverview(frame: boolean) {
    this.clearOverview(true);
    this.bedGroup.visible = false;
    this.buildOverview(frame);
  }

  private createPlateVisual(plate: ViewportPlate, center: [number, number]) {
    const group = new THREE.Group();
    group.userData.plateId = plate.id;
    const [bedX, bedY] = this.bedMm;
    const grid = new THREE.GridHelper(
      Math.max(bedX, bedY),
      Math.max(1, Math.round(Math.max(bedX, bedY) / 10)),
      this.dark ? 0x555555 : 0xaaaaaa,
      this.dark ? 0x333333 : 0xcccccc,
    );
    grid.rotation.x = Math.PI / 2;
    grid.scale.set(bedX / Math.max(bedX, bedY), 1, bedY / Math.max(bedX, bedY));
    grid.position.set(center[0], center[1], 0);
    group.add(grid);

    const plane = new THREE.PlaneGeometry(bedX, bedY);
    const outlineGeometry = new THREE.EdgesGeometry(plane);
    plane.dispose();
    const outline = new THREE.LineSegments(
      outlineGeometry,
      new THREE.LineBasicMaterial({ color: this.gridOutlineColor() }),
    );
    outline.position.set(center[0], center[1], 0.02);
    group.add(outline);

    const hitArea = new THREE.Mesh(
      new THREE.PlaneGeometry(bedX, bedY),
      new THREE.MeshBasicMaterial({ transparent: true, opacity: 0, depthWrite: false }),
    );
    hitArea.position.set(center[0], center[1], -0.01);
    hitArea.userData.plateId = plate.id;
    group.add(hitArea);

    const render = this.buildMesh(plate, plate.id === this.selectedPlateId);
    if (render) {
      render.mesh.userData.plateId = plate.id;
      this.placeOnBed(render.mesh, plate.stats!, center);
      group.add(render.mesh);
    }
    group.add(this.createPlateLabel(plate.name, plate.id, center, bedY));
    return {
      group,
      rendered: { id: plate.id, input: plate, outline, render },
    };
  }

  private createPlateLabel(name: string, id: number, center: [number, number], bedY: number) {
    const trimmedName = name.trim();
    const number = id + 1;
    const label =
      trimmedName && trimmedName !== `Plate ${number}`
        ? `Plate ${number} · ${trimmedName}`
        : `Plate ${number}`;
    const canvas = document.createElement("canvas");
    canvas.width = 512;
    canvas.height = 64;
    const context = canvas.getContext("2d")!;
    context.fillStyle = this.dark ? "#e8e8e8" : "#202020";
    context.font = "600 32px system-ui, sans-serif";
    context.textAlign = "center";
    context.textBaseline = "middle";
    let shown = label.slice(0, 64);
    while (shown.length > 1 && context.measureText(shown).width > 480) shown = shown.slice(0, -1);
    if (shown !== label) shown = shown.slice(0, -1) + "…";
    context.fillText(shown, canvas.width / 2, canvas.height / 2);
    const texture = new THREE.CanvasTexture(canvas);
    texture.colorSpace = THREE.SRGBColorSpace;
    const sprite = new THREE.Sprite(
      new THREE.SpriteMaterial({ map: texture, transparent: true, depthTest: false }),
    );
    sprite.position.set(center[0], center[1] - bedY / 2 - 16, 4);
    sprite.scale.set(Math.min(224, this.bedMm[0]), Math.min(224, this.bedMm[0]) / 8, 1);
    sprite.renderOrder = 5;
    sprite.userData.plateId = id;
    return sprite;
  }

  private gridOutlineColor() {
    return this.dark ? 0x555555 : 0x8f8f8f;
  }

  private unselectedModelColor() {
    return this.dark ? 0x3f7568 : 0x4b9982;
  }

  private plateAtPointer(event: PointerEvent): number | null {
    if (!this.overviewGroup) return null;
    const rect = this.renderer.domElement.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const pointer = new THREE.Vector2(
      ((event.clientX - rect.left) / rect.width) * 2 - 1,
      -((event.clientY - rect.top) / rect.height) * 2 + 1,
    );
    const raycaster = new THREE.Raycaster();
    raycaster.setFromCamera(pointer, this.camera);
    for (const hit of raycaster.intersectObject(this.overviewGroup, true)) {
      let object: THREE.Object3D | null = hit.object;
      while (object) {
        const id = object.userData.plateId;
        if (typeof id === "number") return id;
        object = object.parent;
      }
    }
    return null;
  }

  private frameCamera(stats: GeometryStats) {
    const dims = stats.dimensions.map((dimension) => Math.max(dimension, DIMENSION_FLOOR_MM)) as [
      number,
      number,
      number,
    ];
    this.frameDimensions(
      new THREE.Vector3(this.bedMm[0] / 2, this.bedMm[1] / 2, dims[2] / 2),
      dims,
    );
  }

  private frameDimensions(
    center: THREE.Vector3,
    dimensions: [number, number, number],
    preserveDirection = false,
  ) {
    this.frameBounds = { center: center.clone(), dimensions: [...dimensions] };
    const distance = plateCameraDistance(dimensions, this.camera.fov, this.camera.aspect);
    const direction = preserveDirection
      ? this.camera.position.clone().sub(this.controls.target).normalize()
      : new THREE.Vector3(0.6, -0.9, 0.7).normalize();
    this.camera.position.copy(center).addScaledVector(direction, distance);
    this.controls.target.copy(center);
    this.camera.near = Math.max(distance / 100, 0.01);
    this.camera.far = distance * 100;
    this.camera.updateProjectionMatrix();
    this.controls.update();
  }

  dispose() {
    this.resizeObserver.disconnect();
    window.removeEventListener("gogglelab-theme-change", this.onThemeChange);
    window.removeEventListener("gogglelab-accent-change", this.onAccentChange);
    this.renderer.domElement.removeEventListener("pointerdown", this.onPointerDown);
    this.renderer.domElement.removeEventListener("pointerup", this.onPointerUp);
    this.renderer.domElement.removeEventListener("pointercancel", this.onPointerCancel);
    this.clearMesh();
    this.clearOverview();
    this.clearGroup(this.bedGroup);
    disposeObject(this.axes);
    this.renderer.setAnimationLoop(null);
    this.renderer.dispose();
    this.container.removeChild(this.renderer.domElement);
  }
}
