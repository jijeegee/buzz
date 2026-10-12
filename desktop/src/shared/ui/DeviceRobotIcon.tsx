import { Bot } from "lucide-react";
import { useId } from "react";

import type {
  DeviceRobotPaint,
  DeviceRobotVariant,
} from "@/shared/lib/deviceRobot";
import {
  DEVICE_ROBOT_GEOMETRY,
  type DeviceRobotPrimitive,
} from "@/shared/lib/deviceRobotGeometry";
import { cn } from "@/shared/lib/cn";

function primitive(shape: DeviceRobotPrimitive, index: number, ink: string) {
  switch (shape.k) {
    case "rect":
      return (
        <rect
          height={shape.h}
          key={index}
          rx={shape.r}
          width={shape.w}
          x={shape.x}
          y={shape.y}
        />
      );
    case "line":
      return (
        <line
          key={index}
          x1={shape.x1}
          x2={shape.x2}
          y1={shape.y1}
          y2={shape.y2}
        />
      );
    case "circle":
      return <circle cx={shape.cx} cy={shape.cy} key={index} r={shape.r} />;
    case "dot":
      return (
        <circle
          cx={shape.cx}
          cy={shape.cy}
          fill={ink}
          key={index}
          r={shape.r}
          stroke="none"
        />
      );
    case "poly":
      return <polygon key={index} points={shape.points.join(" ")} />;
  }
}

/**
 * A device's robot, or the default `Bot` when `variant` is `null`. Sized by
 * `className` like any lucide icon. Decorative unless `label` is given.
 */
function ink(paint: DeviceRobotPaint, gradientId: string) {
  return paint.kind === "gradient" ? `url(#${gradientId})` : "currentColor";
}

/** A gradient paint as an SVG `linearGradient` in grid coordinates. */
function PaintGradient({
  id,
  paint,
}: {
  id: string;
  paint: Extract<DeviceRobotPaint, { kind: "gradient" }>;
}) {
  return (
    <defs>
      <linearGradient
        gradientUnits="userSpaceOnUse"
        id={id}
        x1={paint.from[0]}
        x2={paint.to[0]}
        y1={paint.from[1]}
        y2={paint.to[1]}
      >
        {paint.stops.map(([offset, color]) => (
          <stop key={`${offset}-${color}`} offset={offset} stopColor={color} />
        ))}
      </linearGradient>
    </defs>
  );
}

export function DeviceRobotIcon({
  variant,
  className,
  label,
}: {
  variant: DeviceRobotVariant | null;
  className?: string;
  /** Accessible name; omit when adjacent text already names the device. */
  label?: string;
}) {
  const gradientId = `device-robot-paint-${useId().replace(/:/g, "")}`;
  if (!variant) {
    return label ? (
      <Bot aria-label={label} className={className} role="img" />
    ) : (
      <Bot aria-hidden="true" className={className} />
    );
  }
  return (
    <svg
      aria-hidden={label ? undefined : true}
      aria-label={label}
      className={cn("shrink-0", className)}
      data-robot-shape={variant.shape}
      data-robot-tag={variant.tag}
      data-testid="device-robot-icon"
      fill="none"
      role={label ? "img" : "presentation"}
      data-robot-paint={variant.paint.name}
      stroke={ink(variant.paint, gradientId)}
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth={2}
      style={{ color: variant.color }}
      viewBox="0 0 24 24"
      xmlns="http://www.w3.org/2000/svg"
    >
      {label ? <title>{label}</title> : null}
      {variant.paint.kind === "gradient" ? (
        <PaintGradient id={gradientId} paint={variant.paint} />
      ) : null}
      {DEVICE_ROBOT_GEOMETRY[variant.shape].map((shape, index) =>
        primitive(shape, index, ink(variant.paint, gradientId)),
      )}
    </svg>
  );
}
