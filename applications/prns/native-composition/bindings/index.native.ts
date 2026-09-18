import "@ubjs/react-native";
import bindings from "./typescript/prns_app";

bindings.initialize();
export * from "./typescript/prns_app";
export { default as bindingModule } from "./typescript/prns_app";
export * from "./typescript/binding-contract.generated";
