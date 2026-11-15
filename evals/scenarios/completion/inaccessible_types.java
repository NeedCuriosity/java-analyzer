//- /lib/Registry.java
package lib;

public class Registry {
    private static class RegistryPrivate {}
    protected static class RegistryProtected {}
    static class RegistryPackage {}
    public static class RegistryPublic {}
}
//- /lib/RegistryHelper.java
package lib;

class RegistryHelper {}
//- /app/Main.java
package app;

class Main {
    void lang() {
        AbstractS/*@abst*/
    }

    Object afterNew() {
        return new Asser/*@after_new*/;
    }

    void nested() {
        lib.Registry.Registry/*@nested*/
    }

    void unimported() {
        Registr/*@unimported*/
    }
}
//- /app/Sub.java
package app;

class Sub extends lib.Registry {
    void f() {
        Registry/*@sub*/
    }
}
//- /lib/Neighbor.java
package lib;

class Neighbor {
    void f() {
        Registr/*@same_package*/
    }
}
//? complete abst lacks AbstractStringBuilder
//? complete after_new has AssertionError
//? complete after_new lacks AssertionStatusDirectives
//? complete nested has RegistryPublic
//? complete nested lacks RegistryPrivate RegistryPackage RegistryProtected
//? complete unimported has Registry
//? complete unimported lacks RegistryHelper
//? complete sub has RegistryProtected RegistryPublic
//? complete sub lacks RegistryPrivate RegistryPackage
//? complete same_package has RegistryHelper RegistryPackage RegistryProtected
//? complete same_package lacks RegistryPrivate
