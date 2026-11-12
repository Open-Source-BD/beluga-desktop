import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";

// Drafts and settings live in localStorage; no test may see another's.
beforeEach(() => localStorage.clear());
afterEach(cleanup);
