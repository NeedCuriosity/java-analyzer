package app;

class Outer {
    private int /*@outer_field*/counter;
    static int /*@static_field*/LIMIT = 10;

    enum Color { /*@red*/RED, /*@green*/GREEN }

    static class /*@nested*/Nested {
        int /*@nested_value*/value = /*@use_static*/LIMIT;
    }

    class /*@inner*/Inner {
        int read() { return /*@use_outer*/counter + Outer.this./*@use_outer_this*/counter; }
    }

    int run(Color color) {
        Outer./*@use_nested*/Nested nested = new /*@new_nested*/Nested();
        /*@use_inner*/Inner inner = new Inner();
        switch (color) {
            case /*@case_red*/RED:
                return nested./*@use_value*/value;
            default:
                return Color./*@use_green*/GREEN.ordinal() + inner.read();
        }
    }
}
//? def use_static -> static_field
//? def use_outer -> outer_field
//? def use_outer_this -> outer_field
//? def use_nested -> nested
//? def use_inner -> inner
//? def case_red -> red
//? def use_value -> nested_value
//? def use_green -> green
//? diag none
