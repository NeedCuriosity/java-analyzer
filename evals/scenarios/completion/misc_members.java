class Main {
    enum Color { RED, GREEN }

    static class ConstantPool {}
    static class Container {}
    static class Context {}

    private final Con/*@declared*/ cp = null;

    void run(byte[] data, int limit, String name, Color color) {
        byte[] copy = data./*@array*/;
        if (limit > /*@binary*/) {
        }
        Color picked = Color./*@enum_constants*/;
    }
}
//? complete array has clone length
//? complete array first clone
//? complete binary order limit name
//? complete enum_constants order RED values
//? complete enum_constants order GREEN valueOf
//? complete declared first ConstantPool
