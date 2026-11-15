/**
 * Module declaration.
 */
@Deprecated
open module com.example.app {
    requires java.base;
    requires transitive java.sql;
    requires static java.compiler;
    exports com.example.api;
    exports com.example.internal to com.example.test, com.example.tools;
    opens com.example.model to com.fasterxml.jackson.databind;
    uses com.example.spi.Plugin;
    provides com.example.spi.Plugin with com.example.impl.DefaultPlugin, com.example.impl.OtherPlugin;
}
