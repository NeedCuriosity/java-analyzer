interface Shape {
    /*@field_public*/public /*@field_static*/static /*@field_final*/final int SIDES = 0;

    /*@method_public*/public /*@method_abstract*/abstract double area();

    /*@default_public*/public default String name() { return "shape"; }

    private void helper() {}

    static Shape unit() { return () -> 1.0; }

    /*@nested_static*/static class Impl {}
}

/*@interface_abstract*/abstract interface Legacy {}

class Outer {
    /*@enum_static*/static enum Color {
        RED;

        /*@ctor_private*/private Color() {}
    }

    /*@record_static*/static /*@record_final*/final record Point(int x, int y) {}

    public static class Nested {}

    public interface Callback {}

    private static final int LIMIT = 3;

    public abstract static class Base {
        public abstract void run();
    }
}

class Fix {
    interface Api {
        /*@fix*/public void call();
    }
}
//? diag redundant-modifier field_public
//? diag redundant-modifier field_static
//? diag redundant-modifier field_final
//? diag redundant-modifier method_public
//? diag redundant-modifier method_abstract
//? diag redundant-modifier default_public
//? diag redundant-modifier nested_static
//? diag redundant-modifier interface_abstract
//? diag redundant-modifier enum_static
//? diag redundant-modifier ctor_private
//? diag redundant-modifier record_static
//? diag redundant-modifier record_final
//? diag redundant-modifier fix
//? diag only redundant-modifier
//? assist simplify fix ; has "        void call();"
