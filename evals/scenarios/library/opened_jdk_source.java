//- /java/lang/Class.java open
package java.lang;

import java.io.InputStream;
import jdk.internal.loader.BootLoader;
import jdk.internal.reflect.Reflection;

public final class Class<T> {
    public InputStream /*@decl*/getResourceAsStream(String /*@param*/name) {
        /*@use_param*/name = /*@call_resolve*/resolveName(name);
        Module /*@local*/thisModule = /*@call_module*/getModule();
        if (/*@use_local*/thisModule./*@call_named*/isNamed()) {
            String mn = thisModule./*@call_get_name*/getName();
            if (!isOpenToCaller(name, Reflection./*@call_static*/getCallerClass())) {
                return null;
            }
            return /*@boot*/BootLoader.findResourceAsStream(mn, name) == null ? null : null;
        }
        return null;
    }
}
//- /app/Main.java
package app;

class Main {
    Object run() { return String.class./*@main_call*/getResourceAsStream("x"); }
}
//? def call_resolve -> lib java.lang.Class#resolveName
//? def call_module -> lib java.lang.Class#getModule
//? def use_param -> param
//? def use_local -> local
//? def call_named -> lib java.lang.Module#isNamed
//? def call_get_name -> lib java.lang.Module#getName
//? def call_static -> lib jdk.internal.reflect.Reflection#getCallerClass
//? hover call_resolve ~ "resolveName(String name)"
//? hover use_local ~ "Module thisModule"
//? def main_call -> decl
//? refs decl = main_call
