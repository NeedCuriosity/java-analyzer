package app;

class Counter {
    private int /*@field*/count;

    int next(int /*@param*/step) {
        int /*@local*/result = /*@use_field*/count + /*@use_param*/step;
        /*@write_field*/count = /*@use_local*/result;
        return /*@use_local2*/result;
    }
}
//? def use_field -> field
//? def use_param -> param
//? def use_local -> local
//? refs field = use_field write_field
//? refs local = use_local use_local2
//? hover use_field ~ "int count"
//? diag none
