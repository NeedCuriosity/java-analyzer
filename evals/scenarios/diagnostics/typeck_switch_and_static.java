import java.util.List;

class Main {
    sealed interface Shape permits Circle, Square, Group {}
    record Circle(double r) implements Shape {}
    record Square(double side) implements Shape {}
    sealed interface Group extends Shape permits Pair, Many {}
    record Pair(Shape a, Shape b) implements Group {}
    record Many(List<Shape> all) implements Group {}

    sealed static abstract class Node {}
    static final class Leaf extends Node {}
    static final class Branch extends Node {}

    enum Color { RED, GREEN }

    int field;

    double all(Shape s) {
        return switch (s) {
            case Circle c -> c.r();
            case Square q -> q.side();
            case Group g -> 0;
        };
    }

    double nested(Shape s) {
        return switch (s) {
            case Circle(double r) -> r;
            case Square(var side) -> side;
            case Pair p -> 1;
            case Many m -> 2;
        };
    }

    double missingOne(Shape s) {
        return /*@missing_sub*/switch (s) {
            case Circle c -> c.r();
            case Square q -> q.side();
            case Pair p -> 1;
        };
    }

    double guarded(Shape s) {
        return /*@guarded*/switch (s) {
            case Circle c when c.r() > 1 -> c.r();
            case Square q -> q.side();
            case Group g -> 0;
        };
    }

    int objects(Object o) {
        int a = /*@object*/switch (o) {
            case String str -> 1;
            case Integer i -> 2;
        };
        int b = switch (o) {
            case String str -> 1;
            case Object other -> 2;
        };
        return a + b;
    }

    int ints(int n, String s) {
        int a = /*@int_no_default*/switch (n) {
            case 1 -> 1;
            case 2 -> 2;
        };
        int b = /*@string_no_default*/switch (s) {
            case "a" -> 1;
        };
        int c = switch (n) {
            case 1 -> 1;
            default -> 0;
        };
        return a + b + c;
    }

    int nodes(Node n, Color c) {
        int a = switch (n) {
            case Leaf l -> 1;
            case Branch b -> 2;
        };
        /*@stmt_pattern*/switch (n) {
            case Leaf l -> a++;
        }
        switch (c) {
            case RED -> a++;
        }
        switch (c) {
            case RED: a++;
        }
        return a;
    }

    static int statics() {
        int a = /*@this_field*/this.field;
        Object o = /*@this_ref*/this;
        String s = /*@super_call*/super.toString();
        return a + o.hashCode() + s.length();
    }

    int notStatic() {
        Runnable r = () -> System.out.println(this.field);
        r.run();
        return this.field + super.hashCode();
    }

    void voids() {
        int x = /*@void_init*/helper();
        helper();
    }

    void helper() {}
}

class Hider extends Main {
    static void /*@static_hides*/helper() {}
    private int /*@private_narrows*/notStatic() { return 0; }
}
//? diag non-exhaustive-switch missing_sub
//? diag non-exhaustive-switch guarded
//? diag non-exhaustive-switch object
//? diag non-exhaustive-switch int_no_default
//? diag non-exhaustive-switch string_no_default
//? diag non-exhaustive-switch stmt_pattern
//? diag static-context this_field
//? diag static-context this_ref
//? diag static-context super_call
//? diag type-mismatch void_init
//? diag invalid-override static_hides
//? diag invalid-override private_narrows
//? diag exact
