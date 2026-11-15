class Main {
    int recordUse() {
        record /*@pdecl*/Point(int /*@xdecl*/x, int y) {
            Point twice() { return new Point(x * 2, y * 2); }
        }
        /*@pointty*/Point p = new Point(1, 2);
        Point q = p.twice();
        return p./*@x*/x() + q.y();
    }

    int enumUse() {
        enum Mode { ON, OFF }
        Mode /*@mode*/mode = Mode./*@on*/ON;
        return mode.ordinal() + Mode.values().length;
    }

    int interfaceUse(int base) {
        interface Read { int /*@getdecl*/get(); }
        Read r = () -> base;
        return r./*@get*/get();
    }

    String classUse(String prefix) {
        class Greeter {
            String greet(String name) { return prefix + name; }
        }
        var /*@greeter*/greeter = new Greeter();
        return greeter.greet("x");
    }

    void sameNameInTwoMethods() {
        record Point(String label) {}
        String s = new Point("a").label();
    }

    void errors() {
        record Pair(int a) {}
        Pair pair = new Pair(1);
        pair./*@missing*/b();
    }
}
//? hover pointty ~ "record Point"
//? hover x ~ "int x()"
//? hover mode ~ "Mode mode"
//? hover on ~ "ON"
//? hover get ~ "int get()"
//? hover greeter ~ "Greeter greeter"
//? def x -> xdecl
//? def pointty -> pdecl
//? def get -> getdecl
//? diag unresolved-method missing
//? diag exact
