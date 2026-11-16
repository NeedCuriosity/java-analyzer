//? module app core lib:json
//? module core
//? module other lib:xml
//- /org/json/Json.java lib:json
package org.json;

public class Json {
    public static String stringify(Object value) { return ""; }
}
//- /org/xml/Xml.java lib:xml
package org.xml;

public class Xml {
    public static String parse(String text) { return text; }
}
//- /core/src/main/java/demo/core/Util.java module:core
package demo.core;

public class Util {
    public static String hello() { return "hi"; }
}
//- /core/src/main/java/demo/core/Helper.java module:core
package demo.core;

public class Helper {
    public static int help() { return 1; }
}
//- /other/src/main/java/demo/other/Secret.java module:other
package demo.other;

import org.xml.Xml;

public class Secret {
    public static String value() { return Xml.parse("x"); }
}
//- /other/src/main/java/demo/concealed/Concealed.java module:other
package demo.concealed;

public class Concealed {}
//- /other/src/main/java/demo/shared/Common.java module:other
package demo.shared;

public class Common {}
//- /core/src/main/java/demo/shared/Common.java module:core
package demo.shared;

public class Common {}
//- /app/src/main/java/demo/app/Main.java module:app
package demo.app;

import demo.core.Util;
import /*@imp*/demo.other.Secret;
import /*@lib*/org.xml.Xml;
import /*@pkg*/demo.other.*;
import demo.shared.Common;
import org.json.Json;

class Main {
    Common common;
    /*@helper*/Helper helper;
    Secret secret;
    /*@hidden*/Concealed hidden;

    void run() {
        Json.stringify(Util.hello());
    }
}
//- /scratch/Loose.java module:none
package scratch;

import demo.other.Secret;
import demo.core.Util;

class Loose {
    String run() { return Secret.value() + Util.hello(); }
}
//? diag unresolved-import imp
//? diag unresolved-import lib
//? diag unresolved-import pkg
//? diag unresolved-type helper
//? diag unresolved-type hidden
//? diag exact
//? assist add_import helper ; has "import demo.core.Helper;"
//? noassist add_import hidden
