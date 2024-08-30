// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore
import { defineApp } from "convex/server";
import envVars from "../../../envVars/convex.config";
import errors from "../../../errors/convex.config";

// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore
const app = defineApp();

app.install(errors);
app.install(envVars);

export default app;