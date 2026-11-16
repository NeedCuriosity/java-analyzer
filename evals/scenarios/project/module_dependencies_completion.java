//? module app core
//? module core
//? module other
//- /core/src/main/java/demo/core/UtilityBelt.java module:core
package demo.core;

public class UtilityBelt {}
//- /other/src/main/java/demo/other/UtilitySecret.java module:other
package demo.other;

public class UtilitySecret {}
//- /app/src/main/java/demo/app/Main.java module:app
package demo.app;

class Main {
    void run() {
        Utility/*@main*/;
    }
}
//- /other/src/main/java/demo/other/Consumer.java module:other
package demo.other;

class Consumer {
    void run() {
        Utility/*@other*/;
    }
}
//? complete main has UtilityBelt
//? complete main lacks UtilitySecret
//? complete other has UtilitySecret
//? complete other lacks UtilityBelt
