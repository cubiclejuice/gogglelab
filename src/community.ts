import { AuthoredColorFeature } from "./lib/authored-color-feature";
import { startApp } from "./main";

startApp({
  createColorFeature: () => new AuthoredColorFeature(),
});
