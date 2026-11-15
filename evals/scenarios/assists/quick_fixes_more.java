//- /app/Main.java
package app;

import java.util.List;

class Main {
    private int /*@ufield*/unusedField = 1;

    private void /*@umethod*/helper() {}

    int locals(List<String> names) {
        int /*@ulocal*/unused = 5;
        String /*@ueffect*/first = names.remove(0);
        return names.size();
    }

    void create() {
        /*@newtype*/Widget w = new Widget("a", 2);
        /*@newiface*/Shape s = null;
    }

    void access(Other o) {
        o./*@vis*/secret();
    }

    void assign() {
        final int count = 1;
        /*@final*/count = 2;
    }

    int mismatch() {
        int /*@vtype*/text = "hello";
        return 0;
    }

    int /*@rtype*/ret() {
        return /*@rval*/"x";
    }

    int /*@noret*/missing(boolean b) {
        if (b) {
            return 1;
        }
    /*@close*/}
}

/*@abs*/class Impl implements Runnable {
}

class Child implements Comparable<Child> {
}
//- /app/Other.java
package app;

class Other {
    private void secret() {}
}
//? assist remove_unused ufield ; lacks "unusedField"
//? assist remove_unused umethod ; lacks "helper"
//? assist remove_unused ulocal ; lacks "int unused = 5"
//? assist remove_unused ueffect ; has "        names.remove(0);\n" ; lacks "String first"
//? assist create_class newtype ; has "//- Widget.java\npackage app;\n\npublic class Widget {\n    public Widget(String string, int i) {\n    }\n}"
//? assist create_interface newiface ; has "public interface Shape {"
//? assist change_visibility vis ; has "    void secret() {}" ; lacks "private void secret"
//? assist remove_final final ; has "        int count = 1;" ; lacks "final int count"
//? assist change_type_to_match vtype ; has "String text = \"hello\";"
//? assist change_type_to_match rval ; has "String ret() {"
//? assist add_return close ; has "        }\n        return 0;\n    }"
//? assist make_class_abstract abs ; has "abstract class Impl implements Runnable"
