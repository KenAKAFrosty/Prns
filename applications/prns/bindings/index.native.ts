// Generated namespace assembly. Domain APIs remain unmodified.
import "personal-rns-expo/bindings";
import "@ubjs/react-native";
import prns_chat_app from "./typescript/prns_chat_app";
import prns_contact_card from "./typescript/prns_contact_card";
import prns_groups from "./typescript/prns_groups";
import prns_groups_core from "./typescript/prns_groups_core";
import prns_groups_service from "./typescript/prns_groups_service";
import prns_groups_store_redb from "./typescript/prns_groups_store_redb";
import prns_identities from "./typescript/prns_identities";
import prns_identities_core from "./typescript/prns_identities_core";
import prns_identities_runtime from "./typescript/prns_identities_runtime";
import prns_identities_uniffi from "./typescript/prns_identities_uniffi";
import prns_service_runtime from "./typescript/prns_service_runtime";
import prns_service_runtime_uniffi from "./typescript/prns_service_runtime_uniffi";
import { appApiVersion } from "./typescript/prns_chat_app";

prns_chat_app.initialize();
prns_contact_card.initialize();
prns_groups.initialize();
prns_groups_core.initialize();
prns_groups_service.initialize();
prns_groups_store_redb.initialize();
prns_identities.initialize();
prns_identities_core.initialize();
prns_identities_runtime.initialize();
prns_identities_uniffi.initialize();
prns_service_runtime.initialize();
prns_service_runtime_uniffi.initialize();
if (appApiVersion() !== 3) {
  throw new Error("PRNS app API version mismatch; rebuild the native application");
}
export * as prns_chat_app from "./typescript/prns_chat_app";
export * as prns_contact_card from "./typescript/prns_contact_card";
export * as prns_groups from "./typescript/prns_groups";
export * as prns_groups_core from "./typescript/prns_groups_core";
export * as prns_groups_service from "./typescript/prns_groups_service";
export * as prns_groups_store_redb from "./typescript/prns_groups_store_redb";
export * as prns_host_uniffi from "./typescript/prns_host_uniffi";
export * as prns_identities from "./typescript/prns_identities";
export * as prns_identities_core from "./typescript/prns_identities_core";
export * as prns_identities_runtime from "./typescript/prns_identities_runtime";
export * as prns_identities_uniffi from "./typescript/prns_identities_uniffi";
export * as prns_service_runtime from "./typescript/prns_service_runtime";
export * as prns_service_runtime_uniffi from "./typescript/prns_service_runtime_uniffi";
