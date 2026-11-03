import { Link } from "expo-router";
import { Button, Heading, Screen, Text } from "../src/ui/primitives";
export default function NotFound() { return <Screen><Heading>Page unavailable</Heading><Text>This link does not identify an available screen.</Text><Link href="/conversations" asChild><Button title="Open conversations" /></Link></Screen>; }
