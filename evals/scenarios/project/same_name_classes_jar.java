//? module app lib:legacy
//? module other
//? module third lib:legacy
//- /com/acme/Config.java lib:legacy
package com.acme;

public class Config {
    public static String legacyName() { return "legacy"; }

    public static class Nested {
        public long legacyNested() { return 1L; }
    }
}
//- /other/src/main/java/com/acme/Config.java module:other
package com.acme;

public class /*@otherConfig*/Config {
    public static int /*@otherOnly*/otherOnly() { return 1; }
}
//- /third/src/main/java/com/acme/Config.java module:third
package com.acme;

public class /*@thirdConfig*/Config {
    public static boolean /*@thirdOnly*/thirdOnly() { return true; }
}
//- /app/src/main/java/demo/app/App.java module:app
package demo.app;

import com.acme.Config;

class App {
    String name = /*@appType*/Config./*@legacy*/legacyName();
    long nested = new Config.Nested().legacyNested();
}
//- /other/src/main/java/demo/other/Use.java module:other
package demo.other;

import com.acme./*@otherImport*/Config;

class Use {
    int n = /*@otherRef*/Config./*@otherUse*/otherOnly();
}
//- /third/src/main/java/demo/third/Own.java module:third
package demo.third;

import com.acme./*@thirdImport*/Config;

class Own {
    boolean b = /*@thirdType*/Config./*@thirdUse*/thirdOnly();
}
//? diag none
//? def legacy -> lib com.acme.Config#legacyName
//? def appType -> lib com.acme.Config
//? def otherUse -> otherOnly
//? def thirdType -> thirdConfig
//? def thirdUse -> thirdOnly
//? hover legacy ~ "legacyName"
//? refs otherConfig = otherImport otherRef
//? refs thirdConfig = thirdImport thirdType
