import {
  Children,
  createContext,
  createElement,
  isValidElement,
  type ComponentType,
  type ReactElement,
  type ReactNode,
  useContext,
} from "react";

import type {
  ComponentList,
  ComponentRegistry,
  ComponentSource,
  PageEnvelope,
  RenderMode,
} from "./protocol.js";

declare module "react" {
  interface Attributes {
    "client:load"?: true;
  }
}

interface IslandRenderContext {
  mode: RenderMode;
  insideIsland: boolean;
  collect?: (component: string, props: unknown, requestedId?: string) => string;
}

const islandRenderContext = createContext<IslandRenderContext>({
  mode: "islands",
  insideIsland: false,
});
const registeredIslandNames = new WeakMap<object, string>();

export interface IslandProps {
  children?: ReactElement;
  id?: string;
}

export function Island({ children, id }: IslandProps): ReactElement {
  const context = useContext(islandRenderContext);
  const child = Children.only(children);
  if (!isValidElement(child) || typeof child.type === "string") {
    throw new Error("Phoenix Island requires one React component child");
  }
  if (context.mode !== "islands") return child;
  if (context.insideIsland) throw new Error("Phoenix islands cannot be nested");

  const name = componentName(child.type as ComponentType<any>);
  const islandId = context.collect?.(name, child.props, id) ?? id ?? name;
  return createElement(
    "div",
    { "data-phoenix-island": islandId, "data-component": name },
    createElement(
      islandRenderContext.Provider,
      { value: { ...context, insideIsland: true } },
      child,
    ),
  );
}

export function PhoenixRenderProvider({
  mode,
  collect,
  children,
}: {
  mode: RenderMode;
  collect?: IslandRenderContext["collect"];
  children?: ReactNode;
}): ReactElement {
  return createElement(
    islandRenderContext.Provider,
    { value: { mode, insideIsland: false, collect } },
    children,
  );
}

export function registerIsland(name: string, Component: ComponentType<any>): void {
  registeredIslandNames.set(Component, name);
}

export function island<Props extends object>(
  Component: ComponentType<Props>,
): ComponentType<Props & { islandId?: string }>;
export function island<Props extends object>(
  componentName: string,
  Component: ComponentType<Props>,
): ComponentType<Props & { islandId?: string }>;
export function island<Props extends object>(
  nameOrComponent: string | ComponentType<Props>,
  explicitComponent?: ComponentType<Props>,
): ComponentType<Props & { islandId?: string }> {
  const Component = typeof nameOrComponent === "string" ? explicitComponent : nameOrComponent;
  if (!Component) throw new Error("Phoenix island component is required");
  const name = typeof nameOrComponent === "string"
    ? nameOrComponent
    : componentName(Component);
  return function PhoenixIsland({ islandId = name, ...props }) {
    registerIsland(name, Component);
    return createElement(
      Island,
      { id: islandId },
      createElement(Component, props as Props),
    );
  };
}

export async function requiredComponent(
  registry: ComponentSource,
  name: string,
  kind: string,
): Promise<ComponentType<any>> {
  const entry = registry[name];
  if (!entry) throw new Error(`Phoenix ${kind} is not registered: ${name}`);
  if (typeof entry === "object" && "load" in entry) {
    const module = await entry.load();
    return module.default;
  }
  return entry;
}

export function componentRegistry(
  components: ComponentSource | ComponentList | undefined,
): ComponentSource {
  if (!components) return {};
  if (!Array.isArray(components)) return components as ComponentRegistry;
  return Object.fromEntries(
    components.map((Component) => [componentName(Component), Component]),
  );
}

export function pageProps(envelope: PageEnvelope): Record<string, unknown> {
  return {
    ...(isRecord(envelope.props) ? envelope.props : { value: envelope.props }),
    phoenix: {
      shared: envelope.shared,
      errors: envelope.errors,
      flash: envelope.flash,
    },
  };
}

function componentName(Component: ComponentType<any>): string {
  const registered = registeredIslandNames.get(Component);
  if (registered) return registered;
  const named = Component as ComponentType<any> & { displayName?: string; name?: string };
  const name = named.displayName || named.name;
  if (!name) {
    throw new Error("Phoenix island components must use a named function or explicit name");
  }
  return name
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
    .toLowerCase();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
